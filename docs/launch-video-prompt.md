# Prompt: Nook launch video

Paste everything below the line into a fresh Claude Code session opened in
this repository.

---

You are a motion designer and front-end engineer. Make a **60-second launch
video for Nook**, the floating glass notch for Windows 10/11 in this
repository. The video must be built as code (Remotion + React + TypeScript),
rendered to MP4, with a voice-over from **Fish Audio** and generated assets
from **OpenRouter**. No screen recordings: every shot of the app is a
**simulated, pixel-faithful replica** animated in code.

## 1. Learn the product first (read before writing anything)

- `README.md`: what Nook is, every tool, hotkeys, exact sizes and timings.
- `crates/ui/src/theme.rs`: the real color tokens, glass values, radii.
- `crates/ui/src` (spring animator, squircle, shell state machine, bento grid):
  the real motion. The pill is **140x34**, it springs into a **568x236**
  panel with **4% overshoot**, tiles stagger **20 ms** after the shell is 60%
  open, content scales **0.96 to 1.0**, press scale **0.97**.
- `site/components/NotchStage.tsx`, `site/components/notchEngine.ts`,
  `site/app/notch.css`: an existing web replica of the notch, monitor,
  wallpapers (`wp-dusk`, `wp-lagoon`, `wp-ember`), taskbar and desktop.
  **Port this replica into Remotion** instead of redrawing from scratch, then
  drive it from `useCurrentFrame()` instead of hover events.
- `site/app/globals.css`: brand tokens. Accent `#6d4cff`, dark bg `#0c0b10`,
  display font Bricolage Grotesque, body Geist, mono Geist Mono, in-app face
  Segoe UI Variable. Icons are Phosphor (`site/public/fonts/ph-*.woff2`).
- `site/app/page.tsx`: brand voice and taglines ("A notch for Windows.",
  "Sixteen tools, one hover away.", "Built to disappear.", "Make your own
  tiles.").

Do not invent features. Every claim on screen or in the voice-over must be
true in the README.

## 2. Look and feel

Modern, sleek, playful. Think Apple keynote product film meets Linear/Raycast
launch videos: dark stage, generous negative space, one idea per shot.

- **Motion language = Nook's own physics.** Use spring easing everywhere
  (`spring()` with slight overshoot, never linear). Things squash, settle and
  glide. Morphs, not cuts: the pill morphs into the panel, a tile grows in
  place into its page, the panel morphs into the next scene's frame.
- **Camera:** a virtual camera over a 4K desktop. Slow push-ins onto the
  notch, quick whip-pans between tools, subtle 3D tilt (max 8 degrees) on the
  monitor in hero shots, depth of field blur on the wallpaper behind glass.
- **Glass:** real `backdrop-filter` blur, 1 px top highlight, rim, faint
  grain. The accent tint follows the wallpaper, as in the app.
- **Typography:** big kinetic headlines in Bricolage Grotesque, word-by-word
  spring reveals, tight tracking. Keyboard shortcuts appear as glossy keycaps
  (`Ctrl` `Alt` `T`) that press down in sync with the action.
- **Fun beats:** a cursor with personality (eases, overshoots, does a small
  happy wiggle after a success), confetti of tiny Phosphor icons when the
  grid first opens, a 0.000% CPU counter that refuses to budge.
- **Sound design:** soft UI clicks, glassy whooshes on morphs, a subtle
  chime on the calendar beat, all synced to frames. Music bed: minimal,
  upbeat electronic, 110-120 BPM, ducked under the voice.
- **Format:** 1920x1080, 60 fps, plus a 1080x1920 vertical cut that reframes
  the same scenes (the notch is naturally centered, so it works).

## 3. Storyboard (60 s)

| Time | Shot | What happens on screen |
|---|---|---|
| 0-4 s | Cold open | Black. A tiny glass pill fades in, shows a clock `9:41` and battery. It breathes once. |
| 4-9 s | The reveal | Camera pulls back: the pill sits on top of a Windows desktop (`wp-dusk`). Cursor glides up, hovers, and the pill **springs open** into the bento grid with overshoot. Tiles stagger in. Title: **"A notch for Windows."** |
| 9-15 s | Text extractor | Keycaps `Ctrl` `Alt` `T`. All monitors freeze (slight desaturate + flash). Cursor drags over text in a frozen video frame; the text lifts off the screen as glowing lines and flies into the clipboard tile. |
| 15-21 s | Color picker | `Ctrl` `Alt` `P`. The loupe follows the cursor over the wallpaper, zooms in with a scroll, snaps on a pixel. The HEX value pops out and cycles HEX / RGB / OKLCH. Accent of the whole UI shifts to that color. |
| 21-27 s | Wallpaper + accent | Click the wallpaper tile: filmstrip slides in. Hover three thumbnails, click one. The desktop cross-fades `wp-dusk` to `wp-lagoon` and the notch's accent follows live. |
| 27-33 s | Phone link | The panel grows to phone size. A phone mirror with rounded corners appears inside the notch; a file is dragged from the desktop onto it, the pill shows a progress ring, done. A notification pulses the pill. |
| 33-40 s | Quick montage | Fast, rhythmic, on the beat, 1.2 s each: Calculator typing `5 km to mi` with the answer appearing as you type; QR code drawing itself while a URL is typed, styles flipping (dots, liquid, rounded); Timer ring counting down mirrored in the pill; Voice memo waveform with time in the pill; System monitor rings. |
| 40-46 s | Built to disappear | Cursor leaves, the panel folds back to the pill. A game goes fullscreen and the pill politely hides. A huge `0.000% CPU` counter, perfectly still. Title: **"Built to disappear."** |
| 46-53 s | Make your own tiles | Split screen: a `plugin.toml` and a few lines of Python typing on the left; on the right a new tile appears live in the grid with no restart. Edit mode: tiles drag and repack with springs. Title: **"Make your own tiles."** |
| 53-60 s | End card | Panel morphs into the logo lockup. **"Nook"**, subline **"Open source. Free. Windows 10 and 11."**, then the website URL and a Get Nook button that gets one satisfying click. Pill shrinks to the top and breathes once, mirroring the opening. |

Keep each tool shot to one clear action. Show the hotkey, the action, the
result. Never hold a static frame longer than 1.5 s.

## 4. Voice-over (Fish Audio)

Use the Fish Audio MCP tools.

1. `get_credit_balance`, then `search_voices` for a warm, confident,
   modern narrator (English, natural, slightly playful, not salesy). Pick
   2-3 candidates with `get_voice`, generate the first line with each via
   `text_to_speech`, and **ask me to pick** before generating the rest.
2. Generate one clip per line (not one long file) so each line can be
   placed on its shot. Save to `video/public/vo/NN.mp3`.
3. Measure each clip's duration and fit the timeline to the voice, not the
   other way round. Leave 0.3-0.6 s of air between lines.

Script (about 120 words; tighten if the timing demands):

1. "This is Nook."
2. "A notch for Windows. Hover, and it opens."
3. "Grab text from anything on screen."
4. "Pick any color, down to the pixel."
5. "Change your wallpaper, and Nook changes with it."
6. "Your phone, right inside the notch. Files, notifications, the whole screen."
7. "A calculator, QR codes, timers, voice memos. Sixteen tools, one hover away."
8. "And when you are done, it folds away. Zero percent CPU."
9. "Want something it does not do? Build your own tile, in any language."
10. "Nook. Free and open source. Get it today."

## 5. Generated assets (OpenRouter)

Use the OpenRouter MCP tools (`list-models` to find a current image model,
then `generate-image`). Generate only what cannot be drawn in code:

- 3 abstract wallpapers matching the existing `wp-dusk`, `wp-lagoon`,
  `wp-ember` palettes, 3840x2160, no text, no logos, smooth gradients with
  soft hills, suitable behind frosted glass.
- 1 "frozen video frame" image with readable English paragraph text for the
  text extractor shot (a fictional article, no real brands).
- 1 fictional phone home screen for the phone mirror (no real app logos).
- 1 fullscreen game still for the "hides behind fullscreen" beat (original,
  no real game IP).
- Optional: a short music bed, if an audio model is available; otherwise
  leave a placeholder track and tell me.

Save to `video/public/assets/`, keep the prompts you used in
`video/assets.md`, and check each image before using it. Regenerate anything
with garbled text, artifacts or real brands. UI, icons, text, the notch and
all motion stay in code, never in generated images.

## 6. Build

- Create the project in `video/` (Remotion, TypeScript). One composition per
  scene plus a `Main` composition that sequences them, and a `Vertical`
  composition for 9:16.
- Put timings in one `timeline.ts` file driven by the measured VO
  durations, so changing a line re-flows the video.
- Reuse the replica's tokens and geometry; keep all sizes in the app's
  logical pixels and scale the stage, so the notch looks exactly like Nook.
- Render stills of every scene (`npx remotion still`) and look at them
  before the full render. Fix anything that does not match the real app.
- Final renders: `video/out/nook-launch-16x9.mp4` and
  `video/out/nook-launch-9x16.mp4`, H.264, -14 LUFS, voice clearly above
  music.

## 7. Before you finish

- Watch-through checklist: every claim is in the README; no frame is empty
  or static too long; text readable at phone size; motion feels like
  springs, not tweens; audio and visuals in sync within 2 frames.
- Tell me what you generated, what each Fish Audio and OpenRouter call cost
  in credits, and anything you could not do.
- Ask me before spending more than about 20 USD in total credits.
