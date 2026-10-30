# Demo video

`pc-agent-demo.mp4` is the 2:55 English demo for the hackathon jury. `youtube-thumbnail.png` is
its 1280×720 thumbnail.

The product shots are recordings of the real interface running on the design mock's synthetic
data (`apps/gui/dev/mock`), not a real PC. The memory price, the specialist's steps and every
measurement are demo data. The "PC cleaner", Task Manager and AI agent terminal screens at the
start are generic mock-ups made for the video, not captures of real products.

## Sources

- `film/` is a [HyperFrames](https://hyperframes.heygen.com) project:
  - `index.html` mounts one scene per file in `compositions/`;
  - `assets/` holds the clips, voice-over, music and sound effects.
- `tools/build.py` writes `film/index.html` and `film/compositions/*.html` from `tools/timing.json`.
  Edit it, not the generated HTML.
- `tools/vo.json` is the voice-over script. `tools/tts.py` and `tools/sfx.py` generate the voice,
  sound effects and music with the ElevenLabs API. They read the key from `ELEVENLABS_API_KEY`
  and never store it.
- `tools/cap-hires.cjs` records the product clips against the mock served on `127.0.0.1:5174`.
  `tools/cap-lib2.cjs` records them with Chromium's screencast at 2880×1620, so the camera can
  zoom in without blurring, and writes the time of each key moment (`tools/marks/`).
  `build.py` uses these times to land each click on the word that describes it.

The tools were run from a scratch folder whose layout differs from this one (`film/`, `vo/`,
`hires/` side by side), so adjust the paths before re-running them.

## Render

```bash
cd docs/video/film
npx --yes hyperframes@0.8.221 check
npx --yes hyperframes@0.8.221 render -q looks -o ../pc-agent-demo.mp4
```
