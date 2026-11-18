# Transparent sidebar mark

`app-mark-transparent.png` is a small transparent derivative of
`app-icon-source.png`, used in the navigation rail and login/status screens. The desktop application
icon and original artwork are unchanged.

Background removal used the built-in `imagegen` tool with this direction:

> Remove only the exterior white background. Preserve the original blue/cyan
> logo, separate sparkle and opaque white interior chat bubble. Keep the exact
> proportions and colors; no redesign, glow, halo, shadow, black background or
> white tile. Output a transparent PNG with clear margins.

The chosen result was cropped to the artwork and exported as a 128px lossless
PNG for offline use, including older WKWebView versions. Transparency is retained.
