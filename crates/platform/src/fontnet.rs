//! Neural font recognition with Lens (<https://github.com/mixfont/lens>): a
//! ResNet18 trained by Mixfont on 983 open-source families (mostly Google
//! Fonts), 94.6% validation accuracy.
//!
//! The network runs on the CPU with candle, straight from Lens's PyTorch
//! checkpoint (candle parses the pickle itself and never runs pickle code).
//! Preprocessing follows Lens's `lens_inference.py`: the word crop is
//! letterboxed into 384x128 on its average border color (bicubic), turned
//! grey, copied to three channels and normalized with the ImageNet mean/std.
//!
//! The weights are not bundled (Lens is licensed for personal and
//! non-commercial use only). Put `font_classifier.pt` and `classes.json`
//! from Lens's `model/` folder in one of the folders [`model_dir`] searches;
//! without them detection falls back to installed-font matching alone.
//!
//! The model is loaded for each detection and dropped afterwards (~0.3 s,
//! 47 MB), so an idle notch does not keep it resident.

use crate::capture::Frame;
use crate::{PlatformError, Result};
use candle_core::{D, DType, Device, Module, ModuleT, Tensor};
use candle_nn::{BatchNorm, Conv2d, Conv2dConfig, Linear, VarBuilder};
use image::{RgbImage, imageops};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const WEIGHTS: &str = "font_classifier.pt";
const CLASSES: &str = "classes.json";
const INPUT_H: u32 = 128;
const INPUT_W: u32 = 384;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// One family and the network's probability for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Prediction {
    pub family: String,
    pub prob: f32,
}

fn err(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Other(format!("lens: {e}"))
}

/// First folder holding the Lens model: `NOOK_FONTNET_DIR`, `fontnet\` next
/// to the exe, `assets\fontnet\` in a parent of the exe (dev builds), then
/// `%APPDATA%\Nook\fontnet\`.
pub fn model_dir() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("NOOK_FONTNET_DIR") {
        dirs.push(d.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("fontnet"));
        }
        dirs.extend(
            exe.ancestors()
                .skip(1)
                .map(|a| a.join("assets").join("fontnet")),
        );
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(a).join("Nook").join("fontnet"));
    }
    dirs.into_iter()
        .find(|d| d.join(WEIGHTS).is_file() && d.join(CLASSES).is_file())
}

struct Block {
    conv1: Conv2d,
    bn1: BatchNorm,
    conv2: Conv2d,
    bn2: BatchNorm,
    down: Option<(Conv2d, BatchNorm)>,
}

fn conv(cin: usize, cout: usize, k: usize, stride: usize, vb: VarBuilder) -> Result<Conv2d> {
    let cfg = Conv2dConfig {
        padding: k / 2,
        stride,
        ..Default::default()
    };
    candle_nn::conv2d_no_bias(cin, cout, k, cfg, vb).map_err(err)
}

fn bn(c: usize, vb: VarBuilder) -> Result<BatchNorm> {
    candle_nn::batch_norm(c, 1e-5, vb).map_err(err)
}

impl Block {
    fn new(cin: usize, cout: usize, stride: usize, vb: VarBuilder) -> Result<Self> {
        let down = if stride != 1 || cin != cout {
            Some((
                conv(cin, cout, 1, stride, vb.pp("downsample.0"))?,
                bn(cout, vb.pp("downsample.1"))?,
            ))
        } else {
            None
        };
        Ok(Self {
            conv1: conv(cin, cout, 3, stride, vb.pp("conv1"))?,
            bn1: bn(cout, vb.pp("bn1"))?,
            conv2: conv(cout, cout, 3, 1, vb.pp("conv2"))?,
            bn2: bn(cout, vb.pp("bn2"))?,
            down,
        })
    }

    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let y = self.bn1.forward_t(&self.conv1.forward(x)?, false)?.relu()?;
        let y = self.bn2.forward_t(&self.conv2.forward(&y)?, false)?;
        let skip = match &self.down {
            Some((c, b)) => b.forward_t(&c.forward(x)?, false)?,
            None => x.clone(),
        };
        (y + skip)?.relu()
    }
}

/// torchvision's ResNet18 with a 983-way head, in eval mode.
struct ResNet18 {
    conv1: Conv2d,
    bn1: BatchNorm,
    blocks: Vec<Block>,
    fc: Linear,
}

impl ResNet18 {
    fn new(classes: usize, vb: VarBuilder) -> Result<Self> {
        let mut blocks = Vec::new();
        let mut cin = 64;
        for (i, cout) in [64, 128, 256, 512].into_iter().enumerate() {
            let stride = if i == 0 { 1 } else { 2 };
            let layer = vb.pp(format!("layer{}", i + 1));
            blocks.push(Block::new(cin, cout, stride, layer.pp("0"))?);
            blocks.push(Block::new(cout, cout, 1, layer.pp("1"))?);
            cin = cout;
        }
        Ok(Self {
            conv1: conv(3, 64, 7, 2, vb.pp("conv1"))?,
            bn1: bn(64, vb.pp("bn1"))?,
            blocks,
            fc: candle_nn::linear(512, classes, vb.pp("fc")).map_err(err)?,
        })
    }

    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let x = self.bn1.forward_t(&self.conv1.forward(x)?, false)?.relu()?;
        // MaxPool(3, stride 2, padding 1). Inputs are >= 0 after the ReLU,
        // so zero padding acts like torch's -inf padding.
        let x = x.pad_with_zeros(2, 1, 1)?.pad_with_zeros(3, 1, 1)?;
        let mut x = x.max_pool2d_with_stride(3, 2)?;
        for b in &self.blocks {
            x = b.forward(&x)?;
        }
        let x = x.mean(D::Minus1)?.mean(D::Minus1)?;
        self.fc.forward(&x)
    }
}

fn load_classes(path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path).map_err(err)?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(err)?;
    let map = json
        .get("class_to_idx")
        .and_then(|m| m.as_object())
        .ok_or_else(|| err("classes.json has no class_to_idx"))?;
    let mut names = vec![String::new(); map.len()];
    for (name, idx) in map {
        let i = idx.as_u64().ok_or_else(|| err("bad class index"))? as usize;
        *names
            .get_mut(i)
            .ok_or_else(|| err("class index out of range"))? = name.clone();
    }
    Ok(names)
}

fn load(dir: &Path) -> Result<(ResNet18, Vec<String>)> {
    let classes = load_classes(&dir.join(CLASSES))?;
    let tensors: HashMap<String, Tensor> =
        candle_core::pickle::read_all_with_key(dir.join(WEIGHTS), Some("model_state"))
            .map_err(err)?
            .into_iter()
            .collect();
    let vb = VarBuilder::from_tensors(tensors, DType::F32, &Device::Cpu);
    Ok((ResNet18::new(classes.len(), vb)?, classes))
}

/// Average color of the outermost pixels.
fn border_mean(img: &RgbImage) -> [u8; 3] {
    let (w, h) = img.dimensions();
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    let mut add = |x: u32, y: u32| {
        for (s, c) in sum.iter_mut().zip(img.get_pixel(x, y).0) {
            *s += u64::from(c);
        }
        n += 1;
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 0..h {
        add(0, y);
        add(w - 1, y);
    }
    sum.map(|s| (s / n.max(1)) as u8)
}

/// Lens's `ResizePadToSize` + `Grayscale(3)` + `Normalize`, as CHW floats.
fn preprocess(crop: &Frame) -> Vec<f32> {
    let rgb = crop.to_rgb_image();
    let (w, h) = rgb.dimensions();
    let fill = border_mean(&rgb);
    let scale = (INPUT_W as f32 / w as f32).min(INPUT_H as f32 / h as f32);
    let nw = ((w as f32 * scale).round() as u32).clamp(1, INPUT_W);
    let nh = ((h as f32 * scale).round() as u32).clamp(1, INPUT_H);
    let resized = imageops::resize(&rgb, nw, nh, imageops::FilterType::CatmullRom);
    let mut canvas = RgbImage::from_pixel(INPUT_W, INPUT_H, image::Rgb(fill));
    imageops::overlay(
        &mut canvas,
        &resized,
        i64::from((INPUT_W - nw) / 2),
        i64::from((INPUT_H - nh) / 2),
    );
    // PIL's "L": ITU-R 601-2 luma.
    let grey: Vec<f32> = canvas
        .pixels()
        .map(|p| {
            let [r, g, b] = p.0.map(f32::from);
            (r * 299.0 + g * 587.0 + b * 114.0) / 1000.0 / 255.0
        })
        .collect();
    (0..3)
        .flat_map(|c| grey.iter().map(move |v| (v - MEAN[c]) / STD[c]))
        .collect()
}

/// Runs Lens on each crop and averages the probabilities. Returns the
/// `top` most likely families, best first.
pub fn classify(dir: &Path, crops: &[Frame], top: usize) -> Result<Vec<Prediction>> {
    if crops.is_empty() {
        return Ok(Vec::new());
    }
    let (model, classes) = load(dir)?;
    let data: Vec<f32> = crops.iter().flat_map(preprocess).collect();
    let n = crops.len();
    let input = Tensor::from_vec(
        data,
        (n, 3, INPUT_H as usize, INPUT_W as usize),
        &Device::Cpu,
    )
    .map_err(err)?;
    let probs = candle_nn::ops::softmax(&model.forward(&input).map_err(err)?, D::Minus1)
        .and_then(|p| p.mean(0))
        .and_then(|p| p.to_vec1::<f32>())
        .map_err(err)?;
    let mut ranked: Vec<(usize, f32)> = probs.into_iter().enumerate().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    Ok(ranked
        .into_iter()
        .take(top)
        .filter_map(|(i, prob)| {
            Some(Prediction {
                family: classes.get(i)?.clone(),
                prob,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxes_into_the_input_size() {
        let crop = Frame {
            origin: (0, 0),
            width: 40,
            height: 20,
            pixels: vec![0xFFFF_FFFF; 800],
        };
        let t = preprocess(&crop);
        assert_eq!(t.len(), 3 * (INPUT_W * INPUT_H) as usize);
        // White everywhere: (1 - mean) / std in each channel.
        assert!((t[0] - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-4);
    }
}
