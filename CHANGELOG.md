# Changelog

All notable changes to Wayfinder are documented here.
## v0.7.6 - 2026-10-03


### Bug Fixes

- **tray:** Center the menu highlight on its row

### Documentation

- **tray:** Add tray menu design

### Features

- **tray:** Draw the tray menu with the engine
## v0.7.5 - 2026-10-02

## v0.7.1 - 2026-10-02


### CI

- **release:** Add a velopack switch to Manual Release
## v0.7.0 - 2026-10-02


### Bug Fixes

- **settings:** Forget a switched-away workspace's state
- **app:** Follow monitor rules only when the setup changes
- **theme:** Show MDL2 icons where Segoe Fluent Icons is missing
- **widgets:** Let nil leave out an optional colour

### CI

- **release:** Auto-bump versions and flag PR preview builds
- **release:** Download the previous release to enable deltas

### Documentation

- **widgets:** Add a widgets guide, ship it everywhere
- **settings:** Render the workspaces page
- Describe workspaces and how they follow desktops
- **settings:** Render the media and photo widgets' pages
- Describe the calendar
- Rework the readme with screenshots and diagrams
- Say only that the classic widgets render on windows
- Split the readme into linked guides

### Features

- **update:** Add Velopack installer and silent auto-update
- **workspace:** Keep several named workspaces
- **platform:** Read and watch virtual desktops
- **app:** Follow virtual desktops and monitor setups
- **settings:** Add a workspaces page
- **widgets:** Add a calendar
- **elements:** Add a 3d block and a graph minimum
- **audio:** Expose a triggered waveform
- **visualizer:** Add oscilloscope and 3d bars
- **elements:** Add a graph area opacity
- **audio:** Keep a second of band history
- **visualizer:** Add a 3d waterfall
- **settings:** Add a pre-release update channel
- **settings:** Add a Version section with update and changelog

### Miscellaneous

- License under mit
## v0.6.0 - 2026-10-01


### Bug Fixes

- **data:** Wake a photo frame for its next slide

### Documentation

- Describe the media, photo and agent widgets
- Update the test count
- Describe the widgets' size families

### Features

- **theme:** Add media and navigation glyphs
- **data:** Add a native gallery source
- **data:** Add a native agents source
- **theme:** Add neon noir, paper and sunset palettes
- **widgets:** Add media, photo and agent widgets
- **data:** Give gallery photos a date and folder name

### Performance

- **data:** Date gallery photos once per listing

### Style

- **widgets:** Restyle media, photo and agent widgets
## v0.5.0 - 2026-09-26


### Features

- Settings redesign, seek bars and crossfades
## v0.4.0 - 2026-09-25


### Bug Fixes

- **elements:** Keep one kind table so lookups share it
- **platform:** Restore default corners when blur goes off
- **settings:** Wrap long descriptions beside remove
- **card:** Let roundness reach blurred widgets
- **edit:** Draw the edit mode overlay
- **edit:** Keep the open size of a collapsed widget
- **data:** Read time zone keys up to their terminator
- **data:** Time world clocks from the shown local time
- **elements:** Smooth graph areas that fill from the right
- **theme:** Keep overridden axes in place
- **text:** Sync font files instead of reloading them
- **code:** Show why a plugin's code cannot run
- **widgets:** Wrap the digital clock's date at narrow widths
- **actions:** Keep numbers and booleans typed in set
- **app:** Stop builds taking .wfplugin files from each other
- **drawer:** Header padding is the gap above the icons

### CI

- Build and test on windows, release on tags
- **release:** Add manual release workflow with changelog
- **release:** Start the changelog on the first release

### Documentation

- Add commit message convention
- Describe card, elements and rust widgets
- **theme:** Add theme cascade design spec
- **theme:** Add size limits and blur-off fix to spec
- **theme:** Add theme cascade plan
- **theme:** Document style tokens and size limits
- **widgets:** Describe size tiers, motion and new data
- Describe plugins
- **adr:** Decide how plugins run code
- Describe plugin code
- Record native sources and update counts
- Update the test count
- Update the test count
- **modules:** Document modules, slots and tiers

### Features

- **drawer:** Add collapsible app drawer widget
- **style:** Add global blur, outlines and header-drag options
- **widgets:** Add a widget seam with toml adapter
- **theme:** Seed a theme guide and claude skill
- **theme:** Declare style tokens and layer them
- **card:** Apply style tokens to every widget
- **theme:** Store style globally and migrate old files
- **settings:** Per-widget style overrides
- **edit:** Add per-widget max size with an off switch
- **edit:** Remove a widget from edit mode
- **edit:** Glide windows when they snap
- **ui:** Glide elements to their new place on reflow
- **data:** Add world clocks to the clock source
- **data:** Add history, drives, commit and network to sys
- **elements:** Add a graph element for history lines
- **widgets:** Show world clocks on a large analog clock
- **widgets:** Add world clocks to a tall digital clock
- **widgets:** Give the system monitor size tiers
- **widgets:** Size tiers for the icon list, folder and drawer
- **widgets:** Resolve ./ image paths next to the file
- **plugins:** Load plugin folders as content roots
- **settings:** Add a plugins page
- **plugins:** Install .wfplugin files
- **app:** Open .wfplugin files from explorer
- **code:** Schedule plugin calls and keep their data
- **code:** Run wasm modules in wasmi
- **net:** Fetch from listed hosts over WinHTTP
- **code:** Serve a data source from a worker
- **plugins:** Run plugin code
- **sdk:** Add the wayfinder-plugin crate
- **data:** Let apps built on wayfinder add sources
- **images:** Decode more formats and play animations
- **images:** Add fit = cover and free unused images
- **plugins:** Allow jpeg, gif, webp and bmp images
- **code:** Let plugin code read declared folders
- **code:** Let plugins list what their widgets open
- **widgets:** Add at(), needs and named missing sources
- **params:** Add groups and labelled choices
- **input:** Take dropped files and scroll sideways
- **cli:** Render widgets and pack or check plugins
- **data:** Let cadence follow source state
- **data:** Let sources and drops save widget params
- **data:** Add a built-in media source
- **data:** Add a built-in audio source
- **images:** Add max for small copies of pictures
- **actions:** Save settings with param from clicks
- **plugins:** Allow several code sources per plugin
- **sysmon:** Add gpu gauges and graphs
- **settings:** Use a native resizable window
- **modules:** Declare tiers, slots and modules in widgets
- **sysmon:** Arrange gauges and graphs as modules
- **settings:** Arrange modules on a live widget preview
- **drawer:** Remove items and accept dropped files
- **drawer:** Header options, fonts and shortcut icons
- **settings:** Restart button for the graphics adapter
- **image:** Feathered edges and a file picker param

### Refactor

- **card:** Centralise gutter, blur and outlines
- **data:** Make each data source a module
- **elements:** One module per element kind
- **drawer:** Make the drawer a rust widget
- **app:** Split app.rs into one module per concern
- **app:** Make instance scheduling and actions testable
- Prefer descriptive names over comments
- **content:** Load content from ordered roots

### Testing

- **widgets:** Lay out every widget at its size limits
- **app:** Let the self-test wait for window glides
- Survive CRLF guides and young clocks
- **settings:** Cover the restart action

### Wayfinder

- A GPU-composited desktop widget engine for Windows
