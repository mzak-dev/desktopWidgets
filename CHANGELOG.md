# Changelog

All notable changes to Wayfinder are documented here.
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
