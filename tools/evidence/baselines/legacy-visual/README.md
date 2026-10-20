# Legacy visual reference (01)

Frames of the Bevy baseline as it renders the frozen qualification scenarios, for the native
client (06) to compare against. They record the presentation and the developer overlay with the
default lighting and quality settings listed in
[acceptance](../../../../docs/development/virtual-office/gpu-transition/acceptance.md#baseline-presentation-and-tuning-reference-01).
Pixel identity is not the goal; a changed look needs a recorded reason.

| Frame | Scenario (SHA-256 prefix) | Tick | Shows |
| --- | --- | ---: | --- |
| `qualification-idle-tick300.jpg` | `qualification/idle.toml` (`d031b681`) | 300 | Generated shore at seed 69: grass, the sand band, the lake, fog towards the window edge. |
| `qualification-coupled-tick900.jpg` | `qualification/coupled.toml` (`03e7f00d`) | 900 | Metal and wood spheres on the sand bank beside the lake, specular water, grass and a pond beyond. |
| `qualification-mixed-tick1300.jpg` | `qualification/mixed.toml` (`1dcabb20`) | 1300 | The lake frozen at −8 °C, ground burnt by the fire front, and the column that rises where the lava pour meets the lake ([source audit](../../../../docs/development/virtual-office/gpu-transition/source-audit.md#found-while-measuring-01)). |

## How they were made

On 2026-10-12, on the nominated machine (RTX 5070 Ti, Vulkan, NVIDIA 615.71.09, KDE Wayland), from
the working tree over `ecd9212`. The renderer and shaders are those of the measured baseline
`6e108d6`; since then only imports changed and the chunk transform uses
`world_model::spatial::cell_render_xz`, which gives the same positions. Each frame comes from one
run of the baseline driver with a new world directory, launched from a scratch working directory:

```sh
target/release/legacy-baseline --scenario tools/evidence/scenarios/qualification/CASE.toml \
    --world-dir NEW_DIR --samples FILE --summary FILE --asset-dir crates/morbid-app/assets \
    --screenshot TICK:CASE-tickTICK.png
```

06 removed the Bevy baseline, its assets and the `legacy-baseline` driver; `9a3230c` is the last
commit that builds them, so reproducing these frames means building that commit.

Every accepted run kept its requested 1920×1080 window. A first mixed run was resized by the
compositor (2560×1368 from frame 659) and was rejected and repeated. The PNGs the driver wrote are
kept on the measuring machine in `target/evidence/visual-reference/`; these files are JPEG copies
(`magick -strip -quality 88`), at the same size.

| PNG | SHA-256 |
| --- | --- |
| `idle/idle-tick300.png` | `98c0c78c9e1d9a4f380b54aa4ebafb428e80d77b71d75429e4851d438396f3c2` |
| `coupled/coupled-tick900.png` | `9c677070a5601376ce1a6bc265516c2058812cafd7a63d678b00d0efac1df976` |
| `mixed/mixed-tick1300.png` | `b4a0cb8141e3d3d9438db7758c49602bccce507f9f68d44012a54de7d7946c2c` |
