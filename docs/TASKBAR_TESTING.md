# Taskbar implementation and manual verification

This is the acceptance checklist for replacing Dook with a native-feeling Nook
taskbar. Keep controls truthful: if an OS capability is unavailable, hide it or
show its unavailable state instead of drawing a nonfunctional control.

## Implementation phases

- [ ] 1. Native taskbar window and responsive visuals: Floating and Bar styles,
  edge placement, monitor targeting, theme glass, DPI sizing, no-activate/tool
  window styles, fullscreen behavior, and functional Start/Search/Task View/
  Clock/Notifications/Show desktop buttons.
- [ ] 2. Event-driven running-window tracking, app grouping, active/minimized
  indicators, window activation/minimize behavior, and attention state.
- [ ] 3. Shell icons for desktop and packaged apps, background loading, neutral
  placeholders, and bounded cache.
- [ ] 4. Ordered persistent pins, import from Explorer, add/remove/reorder,
  launch targets, and per-pin actions.
- [ ] 5. Live supported Wi-Fi, Bluetooth, airplane, audio, battery, and
  brightness controls, plus clock/calendar and status popovers.
- [ ] 6. AppBar work-area reservation, animated auto-hide, and opt-in Windows
  taskbar replacement with restoration on normal exit, panic, and explicit
  recovery command.
- [ ] 7. Responsive settings, migration, accessibility, and documentation.
- [ ] 8. Polish and regression review.

## Manual Windows acceptance

- [ ] Floating taskbar remains inset from the selected screen edge and does not
  reserve work area; Bar taskbar spans the selected monitor and keeps maximized
  windows clear of it.
- [ ] Top and bottom placement work at 100%, 125%, and 150% DPI and across
  monitors with different scaling and negative desktop coordinates.
- [ ] All-monitors, primary-only, and follow-cursor targeting create the right
  set of taskbars and close stale windows after display changes.
- [ ] Dark and light themes render with no opaque white strip, no focus steal,
  and no taskbar/Alt+Tab entry for the Nook taskbar itself.
- [ ] Start, Search, Task View, Notifications, Show desktop, Clock, and every
  visible status control perform their labeled action.
- [ ] Pinned desktop and Store apps show their real icons, launch correctly,
  preserve order, and support the visible pin actions.
- [ ] Running apps restore, activate, minimize, group, and close correctly;
  multiple windows expose their titles without blocking the taskbar.
- [ ] Status controls reflect actual hardware state and omit unavailable radios,
  brightness, or battery features.
- [ ] Auto-hide reveals from the configured edge and stays responsive in
  fullscreen; idle CPU returns to 0% when no animation or state change occurs.
- [ ] Disabling taskbar replacement, quitting normally, forced termination
  recovery, panic recovery, and the restore command leave Windows taskbars in
  their original state.
- [ ] Keyboard navigation, focus states, outside-click/Escape popover closing,
  and reduced-motion behavior work.
