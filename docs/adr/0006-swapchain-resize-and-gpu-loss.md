# Swapchain sizing in buckets, and recovery from GPU loss

wgpu resizes a DirectComposition swapchain with `ResizeBuffers`, and its own source flags that path ("doesn't properly re-initialize all of the things"); it returns "window is in use" when it fails. Reconfiguring on every frame of an animated resize or an Edit Mode drag, with several windows, ended with every surface "not configured" at once and crashed the AMD driver on the development machine (four times).

So:

- The swapchain is sized in 128 px buckets with 128 px of slack, decoupled from the window size. It grows immediately when the window outgrows it and shrinks only when it is 512 px too big for over 600 ms. Everything is drawn from the top-left and the window clips the rest, so nothing is scaled.
- An expand animation reconfigures once, up front, to the target size; the animation only moves the window. A self-test asserts at most one reconfigure.
- A failed configure is retried by creating a fresh surface. Three failures in a row, or a device-lost callback, rebuilds the device and every window's target. Three losses in 90 seconds switch the session to the software renderer instead of looping.
- Automated tests use the software adapter (`--gpu software`, `WAYFINDER_GPU=software`), which never touches the vendor driver.

Not verified on real hardware after the change, deliberately: the previous runs crashed the driver, and this needs the owner's go-ahead.
