# Optional Primary Output Design

Issue #28 needs the main RustJay output window to be optional without changing
the renderer's ownership of its winit surface.

Keep creating the primary window, but pass `visible = false` at creation when
the setting says to hide it. `WgpuEngine` still requires that window to create
its surface, so making the engine truly windowless would require an unrelated
renderer restructure.

Add an early `EffectPlugin` default hook that Vjarda enables when projection is
compiled in. Store `hide_main_output` as an optional application setting: a
missing value in an old or new config uses the plugin default, while either
saved boolean overrides it. Resolve that precedence into `EngineState` before
winit's `resumed` callback creates the window.

Both settings UIs update `EngineState` and request a save. The application loop
detects changes and calls winit's existing visibility API, so no restart is
needed. macOS reopen shows the primary window only when the resolved setting
allows it, while continuing to restore the control window.

Unit-test the saved-setting-versus-plugin-default precedence, including both
override directions. Existing workspace checks cover compilation and tests;
this branch has no rustjay-gui kittest settings harness to extend.
