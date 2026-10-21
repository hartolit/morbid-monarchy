# morbid-app

The native client: the window, input and overlay of Morbid Monarchy, composed over the same
owners the headless evidence runs: `world-store` (the saved world), `world-runtime`'s session
(ticks, residency, checkpoints, receipts), `gpu-world` (the live world on the GPU) and
`world-render` (drawing it on the same device and timeline). The `morbid` executable is the
application; the library lets the evidence runner drive the real client by script.

## Run it

```sh
cargo run --locked --release -p morbid-app --bin morbid
```

The start screen lists the saved worlds, creates new ones (a name and a seed) and imports a save
the Bevy baseline wrote (its seed and elevation scale must be given; the baseline used 69 and
0.5). Worlds are files named `<name>.world` in the user's data directory
(`$XDG_DATA_HOME/morbid-monarchy/worlds`, or `~/.local/share/...`); the settings file is
`$XDG_CONFIG_HOME/morbid-monarchy/client.toml`. Nothing is read or written relative to the working
directory, and shaders and fonts are compiled in, so the client runs the same from anywhere.
`morbid --help` lists the options (`--adapter`, `--worlds`, `--world`, `--settings`, `--window`,
`--vsync`, `--autosave`).

A world is checkpointed every 60 ticks (a second at 60 Hz), on request (F5) and when it closes.
The interval is settable from 60 to 3,600 ticks (`--autosave`, the settings file's
`autosave_ticks`, the world panel) and cannot be turned off: a crash or a lost device loses at most
that interval and the save in progress, which the world panel (ticks since the latest checkpoint)
and the loss report show ([ADR 0004](../../docs/decisions/0004-bounded-loss-by-checkpoint-cadence.md)).

The GPU adapter is chosen explicitly: `--adapter ID`, the settings file's `adapter`, or a choice
on the start screen among the hardware adapters that can present to the window (remembered in
the settings). The world never falls back to another adapter.

## Controls

| Input | Effect |
| --- | --- |
| W A S D, Space, Left Ctrl, Left Shift | Fly (or walk); sprint with Shift. While a sphere is possessed: drive it, Space jumps when it is grounded. |
| Right button + mouse | Look around (the cursor is captured while held). |
| Wheel | Tether distance of the possessed sphere's camera (5 to 100). |
| F | Possess the nearest observed sphere, or release it. |
| N / G | Toggle noclip flight / whether the resident window follows the observer. |
| Left button | Use the tool: a brush stroke each tick while held, or drop a sphere. |
| Mouse back / forward | Attract spheres to / lift spheres at the cursor, each tick while held. |
| 1-9, 0, Escape | Select a tool in the tool bar's order; Escape selects none. |
| [ ] / - = | Brush radius / strength. |
| F5 | Save now. |
| F3 | Debug views: the last tick's activity tiles and the spheres' contact state. |
| ` or F12 | Show or hide the overlay. |

## How a frame runs

`client` drives the window's event loop explicitly (`pump_app_events`): each frame lays out the
overlay, turns input into commands (`play`), runs the session's frame (ticks, polling, residency,
checkpoints; never a wait), reads the commands' outcomes, and submits the renderer's and the
overlay's passes on the world's timeline (`GpuWorld::submit_presentation`). Targets are
GPU-resolved rays; walking reads the floor from column queries; possession follows the sphere's GPU
pose in the renderer's camera pass and its latest sample on the CPU. A minimized window draws
nothing and the loop sleeps until the next tick. With `--vsync` the window presents in FIFO mode;
without it in mailbox mode (immediate only where mailbox is missing). Nothing waits for the device:
while both presentation slots are busy and no tick is due that the device could take, a frame would
change nothing, so the loop asks the GPU owner what it would admit (`GpuWorld::poll_admission`) and
waits a quarter of a millisecond at a time in the event wait instead; a tick that falls due still
runs its frame, which draws nothing. Closing saves the last completed tick and reports whether
everything was saved; a device loss shows the latest complete checkpoint and what was lost, and
recovery opens a new session from it on a new device.
