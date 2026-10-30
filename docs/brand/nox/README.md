# Nox, PC Agent's mascot

Nox is a detective owl. Owls investigate and see what others miss. They also keep watch at night,
like PC Agent's watches and daily report, which keep working while the window is hidden or
closed. Her eyes are NVIDIA green, a nod to the Nemotron model she speaks for, and a small chip
sits on her chest. Each pose stands for one feature, for the README, the demo video or the
submission page.

![Nox in ten poses](nox-poses.png)

| File | Use |
| --- | --- |
| `nox-hello.svg` | welcome, setup guide |
| `nox-investigate.svg` | *Find the cause*: Nemotron runs its tools |
| `nox-thinking.svg` | reasoning, answer streaming |
| `nox-live.svg` | Live PC: a check-up with a stethoscope on the PC |
| `nox-watch.svg` | Watches, on night watch with a lantern |
| `nox-report.svg` | daily report |
| `nox-verified.svg` | verified figures |
| `nox-privacy.svg` | *See what leaves*, sharing modes |
| `nox-cleanup.svg` | actions you approve |
| `nox-local.svg` | local mode, without AI |
| `nox-poses.svg`, `nox-poses.png` | the whole set |
| `nox-thumbnail.png` | 1024 × 1024 cover (submission thumbnail, video) |

All poses come from one generator, so they share the same proportions:
`node docs/brand/nox/generate.mjs`. The app keeps its calm two-eyed window mark and also uses
Nox on the welcome screen and conversation headers: hello, investigating, thinking and local.
These four SVGs are bundled locally in `apps/gui/ui/assets/nox`; keep them in sync after
regenerating the brand set. Nox is decorative, never a replacement for verification or model labels.
