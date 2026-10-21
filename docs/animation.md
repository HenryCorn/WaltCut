# Drawing and 2D animation

WaltCut's drawing layer is a transparent, frame-sized canvas over the video tracks. It stores
pressure-sampled strokes in cels; each cel stays visible until the next cel starts. This lets you
hold a pose for several frames or place a new cel on every frame.

## Draw on a sequence

1. Put the playhead where the drawing should begin and choose **Graphics and Titles ▸ New Layer ▸
   New Drawing**. The drawing is placed on a video track above the picture.
2. Select the **Brush** tool (`Shift+P`). Draw directly in the Program monitor. The options bar
   offers **Pen**, **Pencil**, **Marker**, and **Eraser**, a colour and size, pressure input, cel
   mode, and onion-skin controls.
3. Move the playhead to another drawing frame. Choose **New Cel** for a blank pose or **Duplicate**
   to copy the held pose, then draw the changes. With **Draw on held cel**, strokes instead extend
   the cel already visible on that frame.
4. Enable **Onion skin** to show the neighbouring cels: the previous cel in red and the next in
   green.
5. To fill a gap between two cels, choose **In-between**. It interpolates matching strokes by
   resampling their points; adjust the spacing in the options bar (two frames by default). This is
   a straight stroke interpolation, not a semantic or AI-generated pose.

Drawing clips are ordinary video clips for timeline placement, so clip motion and opacity can be
used to position or fade a drawing.

## Brushes and input

- **Pen** uses pressure for width; **Pencil** uses pressure for width and opacity; **Marker** is
  broad and translucent; **Eraser** removes strokes in the current cel.
- A mouse uses full pressure. Pen pressure is used when the platform supplies it; the options bar
  shows the current input and pressure when available.
- The lower barrel button erases while drawing when the tablet driver maps it to the secondary
  mouse button.

## AI Restyle

Open **Graphics and Titles ▸ AI Restyle…** to restyle the frame at the playhead, every Nth frame of
the sequence In/Out range, or every Nth frame of the selected clip. WaltCut renders the composed
frame—including its picture, graphics, and drawings—and sends it to the configured image provider.
Completed images are imported onto a new video track above the sequence.

The implemented provider is fal.ai cloud; it requires a fal.ai API key and a cloud-enabled build.
The image data leaves the machine when sent to the provider, and cloud requests may incur charges.
The Local Stable Diffusion option is not implemented yet. A fixed seed can help keep the look
consistent, but temporal consistency between generated frames is not guaranteed.

## Current scope

Drawings support raster-rendered Pen, Pencil, Marker, and Eraser strokes, held or duplicated cels,
onion skins, and straight in-between interpolation. This is a focused 2D drawing workflow, not a
full vector animation or rigging system. The general editing engine, formats, and non-animation
features are inherited from FilmCraft; see the [upstream project](https://github.com/storytold/filmcraft)
and the [architecture documentation](architecture.md).
