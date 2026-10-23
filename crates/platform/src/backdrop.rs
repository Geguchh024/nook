//! Shape-clipped acrylic backdrop via Windows.UI.Composition.
//!
//! A `DesktopWindowTarget` hosts a container with three layers: a sprite
//! painted with the host backdrop brush (the blurred desktop behind the
//! window, the same source acrylic uses), a sprite with the glass tint (a
//! vertical gradient), and a shape visual with the 1 px rim and the top
//! highlight. One Direct2D path clip on the container cuts all of them to
//! the shell outline, so everything that defines the *edge* lives in one
//! compositor commit.
//!
//! OpenGL (the main window) and Composition present on separate paths that
//! cannot be synchronized; whenever an edge element was drawn in GL it ran a
//! frame or two ahead of the clip, and a pale ring flashed around the shape
//! while it animated. GL now draws only content well inside the edge.
//!
//! The host window never gets a window region. `SetWindowRgn` takes effect
//! in DWM immediately while the clip arrives with the next compositor
//! commit, and for those one or two frames DWM filled the strip between the
//! new region edge and the old clip with a pale color: the white corners seen
//! while the shell sprang open. The window is instead fully click-through
//! (`WS_EX_LAYERED | WS_EX_TRANSPARENT`) and only the clip defines the shape;
//! the main window's region does the hit-testing.
//!
//! The visual lives in its own window placed directly under the main window
//! (the main window is made an *owned* window of it, which keeps the z-order).
//! Attaching the target to the OpenGL window itself does not work: the
//! composition visual is drawn above the GL content whatever `isTopmost`
//! says, hiding the glass tint and tiles.
//!
//! Why not `SetWindowCompositionAttribute` acrylic: on current Windows 11
//! builds it fills the whole window rectangle and ignores the window region
//! (see docs/NOTES.md).

use crate::window::Hwnd;
use crate::{Result, Win32Context};
use windows::Graphics::IGeometrySource2D;
use windows::System::{DispatcherQueue, DispatcherQueueController};
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::{
    CompositionColorBrush, CompositionColorGradientStop, CompositionLineGeometry,
    CompositionLinearGradientBrush, CompositionMappingMode, CompositionPath,
    CompositionSpriteShape, Compositor, ContainerVisual, SpriteVisual,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_FILL_MODE_WINDING,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1CreateFactory, ID2D1Factory, ID2D1Geometry,
};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGeometrySource2DInterop, IGeometrySource2DInterop_Impl,
};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_HWNDPARENT, GetWindowRect, LWA_ALPHA,
    RegisterClassExW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, Interface, Ref, w};
use windows_core::implement;
use windows_numerics::Vector2;

/// Hands a Direct2D geometry to Composition (`CompositionPath::Create`).
#[implement(IGeometrySource2D, IGeometrySource2DInterop)]
struct GeometrySource(ID2D1Geometry);

impl windows::Graphics::IGeometrySource2D_Impl for GeometrySource_Impl {}

impl IGeometrySource2DInterop_Impl for GeometrySource_Impl {
    fn GetGeometry(&self) -> windows::core::Result<ID2D1Geometry> {
        Ok(self.0.clone())
    }

    fn TryGetGeometryUsingFactory(
        &self,
        _factory: Ref<ID2D1Factory>,
    ) -> windows::core::Result<ID2D1Geometry> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: default handling for a window we created.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Creates the borderless, non-activating host window for the blur.
fn create_host_window() -> Result<HWND> {
    let class = w!("NookBackdrop");
    // SAFETY: standard class registration and window creation; a repeated
    // registration fails harmlessly and CreateWindowExW then uses the class.
    unsafe {
        let module = GetModuleHandleW(None).api("GetModuleHandleW")?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_TOPMOST,
            class,
            w!("Nook backdrop"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
        .api("CreateWindowExW(backdrop)")?;
        // Layered + transparent: clicks pass through to whatever is below.
        SetLayeredWindowAttributes(hwnd, Default::default(), 255, LWA_ALPHA)
            .api("SetLayeredWindowAttributes(backdrop)")?;
        Ok(hwnd)
    }
}

/// Per-frame glass look. Colors are unmultiplied sRGBA; coordinates are
/// window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    /// Fill gradient, top and bottom of the shape.
    pub top: [u8; 4],
    pub bottom: [u8; 4],
    pub y0: f32,
    pub y1: f32,
    /// Show the blur layer under the tint (off for the "Solid" material).
    pub blur: bool,
    /// Rim stroke along the outline. Half of it falls outside the clip, so
    /// pass twice the visible width.
    pub rim: [u8; 4],
    pub rim_width: f32,
    /// Top highlight line: color at its middle, fading to clear at the ends.
    pub highlight: [u8; 4],
    pub highlight_x0: f32,
    pub highlight_x1: f32,
    pub highlight_y: f32,
}

fn color([r, g, b, a]: [u8; 4]) -> Color {
    Color {
        A: a,
        R: r,
        G: g,
        B: b,
    }
}

/// Lives on the UI thread (COM objects and the host window are not `Send`).
pub struct Backdrop {
    _queue: Option<DispatcherQueueController>,
    compositor: Compositor,
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    blur: SpriteVisual,
    tint: CompositionLinearGradientBrush,
    stops: [CompositionColorGradientStop; 2],
    rim: CompositionSpriteShape,
    rim_brush: CompositionColorBrush,
    highlight_line: CompositionLineGeometry,
    highlight_brush: CompositionLinearGradientBrush,
    highlight_stops: [CompositionColorGradientStop; 3],
    last_tint: Option<Tint>,
    factory: ID2D1Factory,
    host: Hwnd,
    placed: Option<(i32, i32, i32, i32)>,
    visible: bool,
}

impl Backdrop {
    /// Creates the backdrop window under `main` and makes it `main`'s owner.
    pub fn new(main: Hwnd) -> Result<Self> {
        // Composition needs a DispatcherQueue on the calling thread.
        let queue = if DispatcherQueue::GetForCurrentThread().is_ok() {
            None
        } else {
            let opts = DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            };
            // SAFETY: options are fully initialized.
            Some(
                unsafe { CreateDispatcherQueueController(opts) }
                    .api("CreateDispatcherQueueController")?,
            )
        };

        let host = Hwnd::from_raw(create_host_window()?.0 as isize);

        let on = BOOL::from(true);
        // SAFETY: attribute and size match.
        unsafe {
            DwmSetWindowAttribute(
                host.get(),
                DWMWA_USE_HOSTBACKDROPBRUSH,
                &on as *const BOOL as *const _,
                std::mem::size_of::<BOOL>() as u32,
            )
        }
        .api("DwmSetWindowAttribute(USE_HOSTBACKDROPBRUSH)")?;

        let compositor = Compositor::new().api("Compositor::new")?;
        let interop: ICompositorDesktopInterop =
            compositor.cast().api("ICompositorDesktopInterop")?;
        // SAFETY: host is our live window.
        let target = unsafe { interop.CreateDesktopWindowTarget(host.get(), true) }
            .api("CreateDesktopWindowTarget")?;
        let full = Vector2 { X: 1.0, Y: 1.0 };
        let root = compositor
            .CreateContainerVisual()
            .api("CreateContainerVisual")?;
        root.SetRelativeSizeAdjustment(full)
            .api("SetRelativeSizeAdjustment")?;

        let blur = compositor.CreateSpriteVisual().api("CreateSpriteVisual")?;
        let backdrop = compositor
            .CreateHostBackdropBrush()
            .api("CreateHostBackdropBrush")?;
        blur.SetBrush(&backdrop).api("SpriteVisual::SetBrush")?;
        blur.SetRelativeSizeAdjustment(full)
            .api("SetRelativeSizeAdjustment")?;

        let clear = Color {
            A: 0,
            R: 0,
            G: 0,
            B: 0,
        };
        let tint = compositor
            .CreateLinearGradientBrush()
            .api("CreateLinearGradientBrush")?;
        tint.SetMappingMode(CompositionMappingMode::Absolute)
            .api("SetMappingMode")?;
        let stops = [
            compositor
                .CreateColorGradientStopWithOffsetAndColor(0.0, clear)
                .api("CreateColorGradientStop")?,
            compositor
                .CreateColorGradientStopWithOffsetAndColor(1.0, clear)
                .api("CreateColorGradientStop")?,
        ];
        let collection = tint.ColorStops().api("ColorStops")?;
        for stop in &stops {
            collection.Append(stop).api("ColorStops::Append")?;
        }
        let tint_visual = compositor.CreateSpriteVisual().api("CreateSpriteVisual")?;
        tint_visual.SetBrush(&tint).api("SpriteVisual::SetBrush")?;
        tint_visual
            .SetRelativeSizeAdjustment(full)
            .api("SetRelativeSizeAdjustment")?;

        // Edge decorations: rim along the outline, highlight along the top.
        let shapes = compositor.CreateShapeVisual().api("CreateShapeVisual")?;
        shapes
            .SetRelativeSizeAdjustment(full)
            .api("SetRelativeSizeAdjustment")?;
        let rim_brush = compositor
            .CreateColorBrushWithColor(clear)
            .api("CreateColorBrush")?;
        let rim = compositor.CreateSpriteShape().api("CreateSpriteShape")?;
        rim.SetStrokeBrush(&rim_brush).api("SetStrokeBrush")?;
        let highlight_brush = compositor
            .CreateLinearGradientBrush()
            .api("CreateLinearGradientBrush")?;
        highlight_brush
            .SetMappingMode(CompositionMappingMode::Absolute)
            .api("SetMappingMode")?;
        let highlight_stops = [
            compositor
                .CreateColorGradientStopWithOffsetAndColor(0.0, clear)
                .api("CreateColorGradientStop")?,
            compositor
                .CreateColorGradientStopWithOffsetAndColor(0.5, clear)
                .api("CreateColorGradientStop")?,
            compositor
                .CreateColorGradientStopWithOffsetAndColor(1.0, clear)
                .api("CreateColorGradientStop")?,
        ];
        let hl_collection = highlight_brush.ColorStops().api("ColorStops")?;
        for stop in &highlight_stops {
            hl_collection.Append(stop).api("ColorStops::Append")?;
        }
        let highlight_line = compositor.CreateLineGeometry().api("CreateLineGeometry")?;
        let highlight = compositor
            .CreateSpriteShapeWithGeometry(&highlight_line)
            .api("CreateSpriteShape")?;
        highlight
            .SetStrokeBrush(&highlight_brush)
            .api("SetStrokeBrush")?;
        let shape_list = shapes.Shapes().api("Shapes")?;
        shape_list.Append(&rim).api("Shapes::Append(rim)")?;
        shape_list
            .Append(&highlight)
            .api("Shapes::Append(highlight)")?;

        let children = root.Children().api("Children")?;
        children.InsertAtTop(&blur).api("InsertAtTop(blur)")?;
        children
            .InsertAtTop(&tint_visual)
            .api("InsertAtTop(tint)")?;
        children.InsertAtTop(&shapes).api("InsertAtTop(shapes)")?;
        // Nothing visible until the first shape arrives.
        root.SetIsVisible(false).api("SetIsVisible")?;
        target.SetRoot(&root).api("DesktopWindowTarget::SetRoot")?;

        // SAFETY: plain factory creation.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .api("D2D1CreateFactory")?;

        // An owned window always stays above its owner, so main sits on the blur.
        // SAFETY: window-long update and show on windows we own.
        unsafe {
            SetWindowLongPtrW(main.get(), GWLP_HWNDPARENT, host.raw());
            let _ = ShowWindow(host.get(), SW_SHOWNOACTIVATE);
        }

        let mut this = Self {
            _queue: queue,
            compositor,
            _target: target,
            root,
            blur,
            tint,
            stops,
            rim,
            rim_brush,
            highlight_line,
            highlight_brush,
            highlight_stops,
            last_tint: None,
            factory,
            host,
            placed: None,
            visible: false,
        };
        this.follow(main)?;
        Ok(this)
    }

    /// The host window (`main`'s owner); z-order changes move both.
    pub fn host(&self) -> Hwnd {
        self.host
    }

    /// Keeps the host window exactly under `main`. Cheap when nothing moved.
    pub fn follow(&mut self, main: Hwnd) -> Result<()> {
        let mut r = RECT::default();
        // SAFETY: valid out pointer.
        unsafe { GetWindowRect(main.get(), &mut r) }.api("GetWindowRect(main)")?;
        let rect = (r.left, r.top, r.right - r.left, r.bottom - r.top);
        if self.placed != Some(rect) {
            // SAFETY: SetWindowPos on our own window.
            unsafe {
                SetWindowPos(
                    self.host.get(),
                    None,
                    rect.0,
                    rect.1,
                    rect.2,
                    rect.3,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
            }
            .api("SetWindowPos(backdrop)")?;
            self.placed = Some(rect);
        }
        Ok(())
    }

    /// Clips everything to `polygon` (window pixels, sub-pixel precise).
    /// Fewer than three points hides it.
    pub fn set_shape(&mut self, polygon: &[(f32, f32)]) -> Result<()> {
        if polygon.len() < 3 {
            return self.set_visible(false);
        }
        let points: Vec<Vector2> = polygon
            .iter()
            .map(|&(x, y)| Vector2 { X: x, Y: y })
            .collect();
        // SAFETY: standard D2D path building on objects we own.
        let geometry: ID2D1Geometry = unsafe {
            let path = self
                .factory
                .CreatePathGeometry()
                .api("CreatePathGeometry")?;
            let sink = path.Open().api("ID2D1PathGeometry::Open")?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLines(&points[1..]);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().api("ID2D1GeometrySink::Close")?;
            path.cast().api("ID2D1Geometry cast")?
        };
        let source: IGeometrySource2D = GeometrySource(geometry).into();
        let path = CompositionPath::Create(&source).api("CompositionPath::Create")?;
        let geom = self
            .compositor
            .CreatePathGeometryWithPath(&path)
            .api("CreatePathGeometryWithPath")?;
        let clip = self
            .compositor
            .CreateGeometricClipWithGeometry(&geom)
            .api("CreateGeometricClipWithGeometry")?;
        self.root.SetClip(&clip).api("SetClip")?;
        self.rim.SetGeometry(&geom).api("SetGeometry(rim)")?;
        self.set_visible(true)
    }

    /// Sets the glass tint and whether the blur shows under it (off for the
    /// "Solid" material). Cheap when unchanged.
    pub fn set_tint(&mut self, tint: Tint) -> Result<()> {
        if self.last_tint == Some(tint) {
            return Ok(());
        }
        self.stops[0].SetColor(color(tint.top)).api("SetColor")?;
        self.stops[1].SetColor(color(tint.bottom)).api("SetColor")?;
        self.tint
            .SetStartPoint(Vector2 { X: 0.0, Y: tint.y0 })
            .api("SetStartPoint")?;
        self.tint
            .SetEndPoint(Vector2 { X: 0.0, Y: tint.y1 })
            .api("SetEndPoint")?;
        if self.last_tint.map(|t| t.blur) != Some(tint.blur) {
            self.blur
                .SetIsVisible(tint.blur)
                .api("SetIsVisible(blur)")?;
        }

        self.rim_brush
            .SetColor(color(tint.rim))
            .api("SetColor(rim)")?;
        self.rim
            .SetStrokeThickness(tint.rim_width)
            .api("SetStrokeThickness")?;

        let [r, g, b, _] = tint.highlight;
        let clear = color([r, g, b, 0]);
        self.highlight_stops[0].SetColor(clear).api("SetColor")?;
        self.highlight_stops[1]
            .SetColor(color(tint.highlight))
            .api("SetColor")?;
        self.highlight_stops[2].SetColor(clear).api("SetColor")?;
        let (x0, x1, y) = (
            tint.highlight_x0,
            tint.highlight_x1.max(tint.highlight_x0),
            tint.highlight_y,
        );
        self.highlight_line
            .SetStart(Vector2 { X: x0, Y: y })
            .api("SetStart")?;
        self.highlight_line
            .SetEnd(Vector2 { X: x1, Y: y })
            .api("SetEnd")?;
        self.highlight_brush
            .SetStartPoint(Vector2 { X: x0, Y: y })
            .api("SetStartPoint")?;
        self.highlight_brush
            .SetEndPoint(Vector2 {
                X: x1.max(x0 + 1.0),
                Y: y,
            })
            .api("SetEndPoint")?;
        self.last_tint = Some(tint);
        Ok(())
    }

    pub fn set_visible(&mut self, visible: bool) -> Result<()> {
        if self.visible != visible {
            self.root.SetIsVisible(visible).api("SetIsVisible")?;
            self.visible = visible;
        }
        Ok(())
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        // SAFETY: destroying the window we created.
        let _ = unsafe { DestroyWindow(self.host.get()) };
    }
}
