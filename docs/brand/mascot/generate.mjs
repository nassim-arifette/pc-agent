// Iris, PC Agent's mascot: the brand mark (an Encre tile with two capsule eyes) given a body.
// Every pose shares one geometry so the set stays consistent. Regenerate with:
//   node docs/brand/mascot/generate.mjs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const OUT = dirname(fileURLToPath(import.meta.url));
const C = {
  ink: "#3B5BDB", inkDark: "#2F49B5", inkLight: "#5C7CFA", eye: "#FFFFFF", pupil: "#1B1B3A",
  cheek: "#FF8FA3", green: "#76B900", greenGlow: "#B5E655", paper: "#FFFFFF", line: "#1B1B1A",
  soft: "#E7ECFF", warn: "#F59F00", good: "#2E9E5B", bad: "#E03131", shadow: "rgba(27,27,26,0.14)",
};

const defs = `
  <defs>
    <linearGradient id="body" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="${C.inkLight}"/><stop offset="1" stop-color="${C.ink}"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.5">
      <stop offset="0" stop-color="${C.greenGlow}" stop-opacity="0.9"/><stop offset="1" stop-color="${C.greenGlow}" stop-opacity="0"/>
    </radialGradient>
  </defs>`;

// Eyes: capsules like the mark, with expressions.
function eyes(kind = "open", dx = 0, dy = 0) {
  const L = 99 + dx, R = 141 + dx, y = 92 + dy;
  switch (kind) {
    case "happy":
      return [L, R].map((x) => `<path d="M${x - 9} ${y + 8} Q${x} ${y - 6} ${x + 9} ${y + 8}" fill="none" stroke="${C.eye}" stroke-width="7" stroke-linecap="round"/>`).join("");
    case "closed":
      return [L, R].map((x) => `<path d="M${x - 9} ${y + 6} Q${x} ${y + 14} ${x + 9} ${y + 6}" fill="none" stroke="${C.eye}" stroke-width="6" stroke-linecap="round"/>`).join("");
    case "focus":
      return [L, R].map((x) => `<rect x="${x - 7}" y="${y + 2}" width="14" height="18" rx="7" fill="${C.eye}"/>`).join("");
    case "wide":
      return [L, R].map((x) => `<rect x="${x - 8}" y="${y - 6}" width="16" height="30" rx="8" fill="${C.eye}"/><circle cx="${x}" cy="${y + 4}" r="3.5" fill="${C.pupil}"/>`).join("");
    default:
      return [L, R].map((x) => `<rect x="${x - 7}" y="${y - 4}" width="14" height="26" rx="7" fill="${C.eye}"/>`).join("");
  }
}

function mouth(kind = "smile") {
  switch (kind) {
    case "o": return `<ellipse cx="120" cy="131" rx="5" ry="6" fill="${C.pupil}" opacity="0.85"/>`;
    case "flat": return `<path d="M112 131 H128" stroke="${C.eye}" stroke-width="4" stroke-linecap="round"/>`;
    case "grin": return `<path d="M108 126 Q120 142 132 126 Z" fill="${C.eye}"/>`;
    default: return `<path d="M110 127 Q120 137 130 127" fill="none" stroke="${C.eye}" stroke-width="4" stroke-linecap="round"/>`;
  }
}

// Arms are round-capped strokes from the shoulders to a small hand.
function arm(side, toX, toY, { hand = true } = {}) {
  const sx = side === "L" ? 62 : 178, sy = 128;
  const cx = side === "L" ? 40 : 200;
  return `<path d="M${sx} ${sy} Q${cx} ${(sy + toY) / 2 + 6} ${toX} ${toY}" fill="none" stroke="${C.inkDark}" stroke-width="11" stroke-linecap="round"/>` +
    (hand ? `<circle cx="${toX}" cy="${toY}" r="9" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3"/>` : "");
}

function body({ eye = "open", eyeDx = 0, eyeDy = 0, mouthKind = "smile", cheeks = true, antenna = true, tilt = 0 }) {
  return `
  <g transform="rotate(${tilt} 120 120)">
    ${antenna ? `<path d="M120 52 V30" stroke="${C.inkDark}" stroke-width="6" stroke-linecap="round"/>
    <circle cx="120" cy="24" r="16" fill="url(#glow)"/><circle cx="120" cy="24" r="8" fill="${C.green}"/>
    <circle cx="117" cy="21" r="2.5" fill="#fff" opacity="0.8"/>` : ""}
    <rect x="70" y="158" width="26" height="22" rx="10" fill="${C.inkDark}"/>
    <rect x="144" y="158" width="26" height="22" rx="10" fill="${C.inkDark}"/>
    <rect x="58" y="50" width="124" height="118" rx="38" fill="url(#body)"/>
    <rect x="58" y="50" width="124" height="118" rx="38" fill="none" stroke="${C.inkDark}" stroke-width="3"/>
    <path d="M78 70 Q86 60 100 58" fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round" opacity="0.35"/>
    ${eyes(eye, eyeDx, eyeDy)}
    ${cheeks ? `<ellipse cx="84" cy="124" rx="8" ry="5" fill="${C.cheek}" opacity="0.55"/><ellipse cx="156" cy="124" rx="8" ry="5" fill="${C.cheek}" opacity="0.55"/>` : ""}
    ${mouth(mouthKind)}
  </g>`;
}

const shadow = `<ellipse cx="120" cy="190" rx="62" ry="9" fill="${C.shadow}"/>`;

const POSES = {
  hello: {
    title: "Hello", note: "Welcome and setup guide",
    svg: () => shadow + arm("L", 46, 160) + body({ eye: "happy", mouthKind: "grin" }) + arm("R", 214, 62) +
      `<path d="M222 40 q8 -6 12 2 M226 54 q10 -2 10 8" fill="none" stroke="${C.inkLight}" stroke-width="4" stroke-linecap="round"/>`,
  },
  investigate: {
    title: "Investigating", note: "Nemotron runs its tools",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "focus", eyeDx: 6, mouthKind: "o" }) +
      arm("R", 196, 118) +
      `<line x1="203" y1="124" x2="222" y2="146" stroke="${C.line}" stroke-width="8" stroke-linecap="round"/>
       <circle cx="190" cy="108" r="22" fill="${C.soft}" fill-opacity="0.7" stroke="${C.line}" stroke-width="6"/>
       <polyline points="176,114 184,104 191,110 202,98" fill="none" stroke="${C.ink}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  thinking: {
    title: "Thinking", note: "Nemotron reasoning, answer streaming",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "open", eyeDx: 5, eyeDy: -6, mouthKind: "flat" }) +
      arm("R", 150, 140) +
      `<circle cx="196" cy="58" r="6" fill="${C.soft}" stroke="${C.inkLight}" stroke-width="3"/>
       <circle cx="210" cy="38" r="9" fill="${C.soft}" stroke="${C.inkLight}" stroke-width="3"/>
       <rect x="196" y="2" width="42" height="28" rx="14" fill="${C.soft}" stroke="${C.inkLight}" stroke-width="3"/>
       <circle cx="208" cy="16" r="3" fill="${C.ink}"/><circle cx="217" cy="16" r="3" fill="${C.ink}"/><circle cx="226" cy="16" r="3" fill="${C.ink}"/>`,
  },
  live: {
    title: "Measuring live", note: "Live PC, refreshed every 5 seconds",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "wide", eyeDx: 4, mouthKind: "smile" }) + arm("R", 186, 118) +
      `<rect x="180" y="58" width="56" height="44" rx="9" fill="${C.paper}" stroke="${C.line}" stroke-width="4"/>
       <polyline points="186,82 196,82 201,70 208,94 214,78 220,82 230,82" fill="none" stroke="${C.good}" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/>
       <path d="M200 102 v12 M216 102 v12" stroke="${C.line}" stroke-width="4" stroke-linecap="round"/>`,
  },
  watch: {
    title: "On watch", note: "A watch threshold was crossed",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "wide", mouthKind: "o", tilt: -4 }) + arm("R", 198, 92) +
      `<path d="M186 70 q12 -26 24 0 v14 l6 8 h-36 l6 -8 z" fill="${C.warn}" stroke="${C.line}" stroke-width="4" stroke-linejoin="round"/>
       <circle cx="198" cy="98" r="5" fill="${C.line}"/>
       <path d="M176 56 q-8 -8 -6 -18 M222 56 q8 -8 6 -18" fill="none" stroke="${C.warn}" stroke-width="4" stroke-linecap="round"/>`,
  },
  report: {
    title: "Daily report", note: "Runs even with the app closed",
    svg: () => shadow + arm("L", 70, 132) + arm("R", 170, 132) +
      body({ eye: "happy", mouthKind: "smile", cheeks: true }) +
      `<rect x="76" y="112" width="88" height="70" rx="8" fill="${C.paper}" stroke="${C.line}" stroke-width="4"/>
       <rect x="104" y="106" width="32" height="12" rx="4" fill="${C.inkDark}"/>
       <path d="M88 132 l6 6 l10 -12" fill="none" stroke="${C.good}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
       <line x1="112" y1="134" x2="152" y2="134" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>
       <path d="M88 152 l6 6 l10 -12" fill="none" stroke="${C.good}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
       <line x1="112" y1="154" x2="146" y2="154" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>
       <circle cx="96" cy="171" r="4" fill="${C.warn}"/><line x1="112" y1="171" x2="140" y2="171" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>
       <circle cx="70" cy="132" r="9" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3"/><circle cx="170" cy="132" r="9" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3"/>`,
  },
  verified: {
    title: "Verified", note: "Every cited figure is checked",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "happy", mouthKind: "grin" }) + arm("R", 204, 84) +
      `<path d="M198 74 v-14 q0 -6 6 -6 q5 0 5 6 v10" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3" stroke-linejoin="round"/>
       <circle cx="34" cy="62" r="22" fill="${C.good}" stroke="${C.line}" stroke-width="4"/>
       <path d="M24 62 l7 7 l13 -15" fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  privacy: {
    title: "Privacy", note: "You choose what leaves the PC",
    svg: () => shadow + arm("L", 52, 160) + body({ eye: "focus", mouthKind: "smile" }) + arm("R", 186, 132, { hand: false }) +
      `<path d="M196 82 l28 10 v22 q0 24 -28 36 q-28 -12 -28 -36 v-22 z" fill="${C.ink}" stroke="${C.line}" stroke-width="4" stroke-linejoin="round"/>
       <rect x="186" y="108" width="20" height="16" rx="3" fill="#fff"/>
       <path d="M190 108 v-5 q0 -7 6 -7 q6 0 6 7 v5" fill="none" stroke="#fff" stroke-width="3.5"/>
       <circle cx="186" cy="132" r="9" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3"/>`,
  },
  cleanup: {
    title: "Approved cleanup", note: "Actions run after your click",
    svg: () => shadow + arm("L", 46, 150) + body({ eye: "happy", mouthKind: "smile" }) + arm("R", 190, 138) +
      `<path d="M186 140 h40 l-5 48 h-30 z" fill="${C.soft}" stroke="${C.line}" stroke-width="4" stroke-linejoin="round"/>
       <rect x="182" y="132" width="48" height="9" rx="4" fill="${C.inkDark}" stroke="${C.line}" stroke-width="3"/>
       <path d="M200 156 l6 -6 l6 6 M206 150 v14 M197 172 q9 8 18 0" fill="none" stroke="${C.good}" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/>
       <circle cx="190" cy="138" r="9" fill="${C.ink}" stroke="${C.inkDark}" stroke-width="3"/>
       <path d="M28 120 l8 -8 M18 132 h10 M30 142 l8 6" stroke="${C.greenGlow}" stroke-width="4" stroke-linecap="round"/>`,
  },
  local: {
    title: "Local mode", note: "No AI, nothing leaves the PC",
    svg: () => shadow + arm("L", 70, 150) + arm("R", 170, 150) + body({ eye: "closed", mouthKind: "smile", antenna: true }).replace(`fill="${C.green}"`, `fill="#9AA3B5"`) +
      `<text x="182" y="52" font-family="Segoe UI, Arial, sans-serif" font-size="22" font-weight="700" fill="${C.inkLight}">z</text>
       <text x="198" y="34" font-family="Segoe UI, Arial, sans-serif" font-size="28" font-weight="700" fill="${C.inkLight}">Z</text>`,
  },
};

const svgFile = (inner, size = 240) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 200" width="${size}" height="${Math.round(size * 200 / 240)}" role="img">${defs}${inner}</svg>\n`;

for (const [name, pose] of Object.entries(POSES)) {
  writeFileSync(join(OUT, `iris-${name}.svg`), svgFile(`<title>Iris, ${pose.title}</title>` + pose.svg()));
}

// A contact sheet with every pose and its caption, light background.
const names = Object.keys(POSES);
const cols = 5, cellW = 260, cellH = 272, rows = Math.ceil(names.length / cols);
const cells = names.map((name, i) => {
  const x = (i % cols) * cellW, y = Math.floor(i / cols) * cellH + 70;
  const pose = POSES[name];
  return `<g transform="translate(${x + 10} ${y})"><rect x="0" y="0" width="${cellW - 20}" height="${cellH - 20}" rx="22" fill="#FFFFFF" stroke="#E4E4E0"/>
    <g transform="translate(0 4)">${pose.svg()}</g>
    <text x="${(cellW - 20) / 2}" y="222" text-anchor="middle" font-family="Segoe UI, Arial, sans-serif" font-size="17" font-weight="700" fill="#1B1B1A">${pose.title}</text>
    <text x="${(cellW - 20) / 2}" y="240" text-anchor="middle" font-family="Segoe UI, Arial, sans-serif" font-size="12" fill="#6B6B66">${pose.note}</text></g>`;
}).join("");
const sheetW = cols * cellW, sheetH = rows * cellH + 80;
writeFileSync(join(OUT, "iris-poses.svg"), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${sheetW} ${sheetH}" width="${sheetW}" height="${sheetH}">${defs}
  <rect width="100%" height="100%" fill="#F3F3F1"/>
  <text x="24" y="44" font-family="Segoe UI, Arial, sans-serif" font-size="28" font-weight="800" fill="#1B1B1A">Iris</text>
  <text x="92" y="44" font-family="Segoe UI, Arial, sans-serif" font-size="16" fill="#6B6B66">PC Agent's mascot, one pose per feature</text>
  ${cells}</svg>\n`);
console.log(`wrote ${names.length} poses and a contact sheet to ${OUT}`);
