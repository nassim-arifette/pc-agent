// Nox, PC Agent's mascot: a detective owl. Owls investigate, see what others miss and keep watch
// at night (watches and the daily report run while the app is closed). Green NVIDIA eyes; a
// circuit-board chest. Every pose shares one geometry. Regenerate: node docs/brand/nox/generate.mjs
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const OUT = dirname(fileURLToPath(import.meta.url));
const C = {
  body: "#26328A", bodyDark: "#1A2366", bodyLight: "#3A4BB8", disc: "#3B5BDB", belly: "#EEF1FF",
  scallop: "#C8D1F7", eye: "#FFFFFF", iris: "#76B900", irisDark: "#4E7D00", pupil: "#0E1230",
  beak: "#F5A524", beakDark: "#C47F0A", feet: "#F5A524", line: "#1B1B1A", paper: "#FFFFFF",
  good: "#2E9E5B", warn: "#F59F00", glow: "#FFE08A", soft: "#E7ECFF", trace: "#76B900",
  shadow: "rgba(16,20,48,0.16)", cheek: "#FF9AB0",
};

const defs = `<defs>
  <linearGradient id="nb" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.bodyLight}"/><stop offset="1" stop-color="${C.body}"/></linearGradient>
  <linearGradient id="nw" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${C.body}"/><stop offset="1" stop-color="${C.bodyDark}"/></linearGradient>
  <radialGradient id="ng" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="${C.glow}" stop-opacity="0.95"/><stop offset="1" stop-color="${C.glow}" stop-opacity="0"/></radialGradient>
  <radialGradient id="ni" cx="0.4" cy="0.35" r="0.7"><stop offset="0" stop-color="#A6E22E"/><stop offset="1" stop-color="${C.irisDark}"/></radialGradient>
</defs>`;

// One eye: white ball, green iris, pupil, highlight; expressions change lids or shape.
function eye(cx, cy, kind, look) {
  const [lx, ly] = look;
  const ball = `<circle cx="${cx}" cy="${cy}" r="21" fill="${C.eye}" stroke="${C.bodyDark}" stroke-width="2.5"/>`;
  const iris = `<circle cx="${cx + lx}" cy="${cy + ly}" r="12" fill="url(#ni)"/><circle cx="${cx + lx}" cy="${cy + ly}" r="6.5" fill="${C.pupil}"/><circle cx="${cx + lx - 4}" cy="${cy + ly - 5}" r="3.2" fill="#fff"/>`;
  switch (kind) {
    case "happy": return `<path d="M${cx - 13} ${cy + 4} Q${cx} ${cy - 12} ${cx + 13} ${cy + 4}" fill="none" stroke="${C.eye}" stroke-width="6" stroke-linecap="round"/>`;
    case "closed": return `<path d="M${cx - 13} ${cy} Q${cx} ${cy + 10} ${cx + 13} ${cy}" fill="none" stroke="${C.eye}" stroke-width="5" stroke-linecap="round"/>`;
    case "focus": return ball + iris + `<path d="M${cx - 22} ${cy - 2} A22 22 0 0 1 ${cx + 22} ${cy - 2} Z" fill="${C.disc}"/><path d="M${cx - 21} ${cy - 2} H${cx + 21}" stroke="${C.bodyDark}" stroke-width="3" stroke-linecap="round"/>`;
    case "wide": return ball.replace('r="21"', 'r="23"') + iris.replace('r="12"', 'r="10"');
    default: return ball + iris;
  }
}

// Wing: a rounded feathered blade from the shoulder to a tip point, so a raised wing really
// touches what it holds. `front` draws it over the body (a wing at the chin, holding a board).
const SHOULDER = { L: [80, 112], R: [160, 112] };
const REST = { L: [60, 170], R: [180, 170] };
function wing(side, tip = REST[side]) {
  const [sx, sy] = SHOULDER[side];
  const [tx, ty] = tip;
  const L = Math.hypot(tx - sx, ty - sy);
  const deg = (Math.atan2(ty - sy, tx - sx) * 180) / Math.PI;
  const k = side === "L" ? 1 : -1;
  const d = `M0 ${-15 * k} C${L * 0.45} ${-21 * k} ${L - 16} ${-14 * k} ${L - 6} ${-9 * k} Q${L + 6} 0 ${L - 6} ${9 * k} C${L - 16} ${13 * k} ${L * 0.45} ${19 * k} 0 ${15 * k} Z`;
  return `<g transform="translate(${sx} ${sy}) rotate(${deg.toFixed(1)})">
    <path d="${d}" fill="url(#nw)" stroke="${C.bodyDark}" stroke-width="2.5" stroke-linejoin="round"/>
    <path d="M${L - 20} ${-5 * k} q7 4 0 ${9 * k} M${L - 32} ${-7 * k} q7 6 0 ${13 * k}" fill="none" stroke="${C.bodyLight}" stroke-width="2" stroke-linecap="round" opacity="0.75"/>
  </g>`;
}

// The rounded tip of a wing shown in front of the body, like a small hand on the chin or a board.
function tipFront(x, y, angle = 0) {
  return `<g transform="translate(${x} ${y}) rotate(${angle})">
    <path d="M-10 -14 C-2 -18 10 -12 11 -2 C12 8 4 15 -4 13 C-12 11 -15 2 -14 -6 Z" fill="url(#nw)" stroke="${C.bodyDark}" stroke-width="2.5" stroke-linejoin="round"/>
    <path d="M-3 -4 q5 3 1 8 M4 -7 q5 4 1 9" fill="none" stroke="${C.bodyLight}" stroke-width="2" stroke-linecap="round" opacity="0.75"/>
  </g>`;
}

function owl({ eyes = "open", look = [0, 0], wingL, wingR, frontWings = false, beak = "closed", blush = false, front = "" } = {}) {
  return `
  <ellipse cx="120" cy="198" rx="58" ry="8" fill="${C.shadow}"/>
  <g fill="${C.feet}" stroke="${C.beakDark}" stroke-width="2" stroke-linejoin="round">
    <path d="M96 184 l-9 12 h8 l1 -5 l3 6 h7 l-3 -12 z"/><path d="M144 184 l9 12 h-8 l-1 -5 l-3 6 h-7 l3 -12 z"/>
  </g>
  ${frontWings ? "" : wing("L", wingL) + wing("R", wingR)}
  <path d="M74 54 L82 22 L100 46 Z" fill="${C.bodyDark}"/><path d="M166 54 L158 22 L140 46 Z" fill="${C.bodyDark}"/>
  <path d="M120 40 C168 40 184 80 184 118 C184 162 158 190 120 190 C82 190 56 162 56 118 C56 80 72 40 120 40 Z" fill="url(#nb)" stroke="${C.bodyDark}" stroke-width="3"/>
  <path d="M80 62 Q94 50 112 48" fill="none" stroke="#fff" stroke-width="4" stroke-linecap="round" opacity="0.25"/>
  <ellipse cx="120" cy="146" rx="40" ry="38" fill="${C.belly}"/>
  ${[[0, 128], [1, 142], [2, 156]].map(([row, y]) => {
    const n = 5 - (row === 2 ? 1 : 0), w = 13, x0 = 120 - (n * w) / 2;
    return Array.from({ length: n }, (_, i) => `<path d="M${x0 + i * w} ${y} q${w / 2} 8 ${w} 0" fill="none" stroke="${C.scallop}" stroke-width="2.5" stroke-linecap="round"/>`).join("");
  }).join("")}
  <g stroke="${C.trace}" stroke-width="2.4" stroke-linecap="round">
    <path d="M108 164 h-6 M108 170 h-6 M108 176 h-6 M132 164 h6 M132 170 h6 M132 176 h6 M114 158 v-5 M120 158 v-5 M126 158 v-5"/>
  </g>
  <rect x="108" y="158" width="24" height="24" rx="5" fill="${C.trace}"/>
  <rect x="113" y="163" width="14" height="14" rx="3" fill="none" stroke="#fff" stroke-width="2" opacity="0.8"/>
  <circle cx="98" cy="94" r="30" fill="${C.disc}"/><circle cx="142" cy="94" r="30" fill="${C.disc}"/>
  ${eye(98, 94, eyes, look)}${eye(142, 94, eyes, look)}
  ${blush ? `<ellipse cx="80" cy="120" rx="7" ry="4" fill="${C.cheek}" opacity="0.6"/><ellipse cx="160" cy="120" rx="7" ry="4" fill="${C.cheek}" opacity="0.6"/>` : ""}
  ${beak === "open"
    ? `<path d="M112 114 L120 110 L128 114 L120 124 Z" fill="${C.beak}" stroke="${C.beakDark}" stroke-width="2" stroke-linejoin="round"/><path d="M114 116 H126" stroke="${C.beakDark}" stroke-width="2"/>`
    : `<path d="M112 113 L120 109 L128 113 L120 126 Z" fill="${C.beak}" stroke="${C.beakDark}" stroke-width="2" stroke-linejoin="round"/>`}
  ${front}
  ${frontWings ? wing("L", wingL) + wing("R", wingR) : ""}`;
}

const POSES = {
  hello: {
    title: "Hello", note: "Welcome and setup guide",
    svg: () => owl({ eyes: "open", look: [2, 1], wingR: [204, 56], beak: "open", blush: true }) +
      `<path d="M212 30 q8 -4 10 4 M220 46 q10 0 8 10" fill="none" stroke="${C.bodyLight}" stroke-width="4" stroke-linecap="round"/>`,
  },
  investigate: {
    title: "Investigating", note: "Nemotron runs its tools",
    svg: () => owl({ eyes: "open", look: [5, 2], wingR: [204, 156] }) +
      `<line x1="198" y1="148" x2="216" y2="172" stroke="${C.line}" stroke-width="8" stroke-linecap="round"/>
       <circle cx="186" cy="132" r="22" fill="${C.soft}" fill-opacity="0.75" stroke="${C.line}" stroke-width="6"/>
       <polyline points="172,138 180,128 187,134 198,122" fill="none" stroke="${C.disc}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  thinking: {
    title: "Thinking", note: "Nemotron reasoning, answer streaming",
    svg: () => owl({ eyes: "open", look: [6, -7], wingR: [176, 150], front: tipFront(140, 132, -20) }) +
      `<circle cx="190" cy="56" r="5" fill="${C.soft}" stroke="${C.bodyLight}" stroke-width="3"/>
       <circle cx="204" cy="38" r="8" fill="${C.soft}" stroke="${C.bodyLight}" stroke-width="3"/>
       <rect x="190" y="2" width="46" height="28" rx="14" fill="${C.soft}" stroke="${C.bodyLight}" stroke-width="3"/>
       <circle cx="203" cy="16" r="3" fill="${C.disc}"/><circle cx="213" cy="16" r="3" fill="${C.disc}"/><circle cx="223" cy="16" r="3" fill="${C.disc}"/>`,
  },
  live: {
    title: "Checking up", note: "Live PC, measured every 5 seconds",
    svg: () => `<g transform="translate(-18 0)">` + owl({ eyes: "focus", look: [6, 3], wingR: [200, 142] }) + `</g>` +
      `<path d="M150 98 C166 104 170 124 164 134" fill="none" stroke="${C.line}" stroke-width="4" stroke-linecap="round"/>
       <circle cx="150" cy="98" r="4" fill="#9AA3B5" stroke="${C.line}" stroke-width="2"/>
       <rect x="188" y="96" width="46" height="88" rx="7" fill="#2B2F3A" stroke="${C.line}" stroke-width="4"/>
       <rect x="196" y="106" width="30" height="6" rx="3" fill="#4A5060"/><rect x="196" y="118" width="30" height="6" rx="3" fill="#4A5060"/>
       <circle cx="211" cy="166" r="5" fill="${C.trace}"/>
       <path d="M164 134 Q170 142 184 140" fill="none" stroke="${C.line}" stroke-width="4" stroke-linecap="round"/>
       <circle cx="188" cy="140" r="8" fill="#C9CED8" stroke="${C.line}" stroke-width="3"/>
       <polyline points="196,82 204,82 208,72 214,92 218,82 230,82" fill="none" stroke="${C.good}" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  watch: {
    title: "On night watch", note: "Watches, even with the window hidden",
    svg: () => `<circle cx="34" cy="34" r="16" fill="${C.glow}"/><circle cx="42" cy="28" r="14" fill="#fff"/>` +
      owl({ eyes: "wide", look: [-3, 0], wingR: [196, 120] }) +
      `<circle cx="198" cy="150" r="28" fill="url(#ng)"/>
       <path d="M198 118 v12" stroke="${C.line}" stroke-width="4" stroke-linecap="round"/>
       <path d="M190 132 h16" stroke="${C.line}" stroke-width="5" stroke-linecap="round"/>
       <rect x="186" y="134" width="24" height="30" rx="6" fill="${C.glow}" fill-opacity="0.85" stroke="${C.line}" stroke-width="4"/>
       <path d="M198 142 q-5 7 0 12 q5 -5 0 -12" fill="${C.warn}"/>`,
  },
  report: {
    title: "Daily report", note: "Runs even with the app closed",
    svg: () => owl({ eyes: "happy", wingL: [66, 160], wingR: [174, 160], blush: true,
      front: `<rect x="82" y="128" width="76" height="62" rx="8" fill="${C.paper}" stroke="${C.line}" stroke-width="4"/>
        <rect x="106" y="122" width="28" height="11" rx="4" fill="${C.bodyDark}"/>
        <path d="M94 146 l5 5 l9 -10" fill="none" stroke="${C.good}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
        <line x1="114" y1="147" x2="146" y2="147" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>
        <path d="M94 164 l5 5 l9 -10" fill="none" stroke="${C.good}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round"/>
        <line x1="114" y1="165" x2="140" y2="165" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>
        <circle cx="100" cy="180" r="4" fill="${C.warn}"/><line x1="114" y1="180" x2="134" y2="180" stroke="${C.soft}" stroke-width="5" stroke-linecap="round"/>` + tipFront(80, 170, 30) + tipFront(160, 170, 150) }),
  },
  verified: {
    title: "Verified", note: "Every cited figure is checked",
    svg: () => owl({ eyes: "happy", wingR: [198, 70], beak: "open", blush: true }) +
      `<circle cx="206" cy="48" r="22" fill="${C.good}" stroke="${C.line}" stroke-width="4"/>
       <path d="M196 48 l7 7 l13 -15" fill="none" stroke="#fff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  privacy: {
    title: "Privacy", note: "You choose what leaves the PC",
    svg: () => owl({ eyes: "focus", look: [0, 2], wingR: [176, 146] }) +
      `<path d="M196 108 l30 11 v24 q0 26 -30 38 q-30 -12 -30 -38 v-24 z" fill="${C.disc}" stroke="${C.line}" stroke-width="4" stroke-linejoin="round"/>
       <rect x="185" y="136" width="22" height="17" rx="3" fill="#fff"/>
       <path d="M189 136 v-6 q0 -8 7 -8 q7 0 7 8 v6" fill="none" stroke="#fff" stroke-width="3.5"/>`,
  },
  cleanup: {
    title: "Approved cleanup", note: "Actions run after your click",
    svg: () => owl({ eyes: "happy", wingR: [190, 142] }) +
      `<path d="M186 146 h40 l-5 46 h-30 z" fill="${C.soft}" stroke="${C.line}" stroke-width="4" stroke-linejoin="round"/>
       <rect x="182" y="138" width="48" height="9" rx="4" fill="${C.bodyDark}" stroke="${C.line}" stroke-width="3"/>
       <path d="M200 162 l6 -6 l6 6 M206 156 v14 M197 178 q9 8 18 0" fill="none" stroke="${C.good}" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round"/>`,
  },
  local: {
    title: "Local mode", note: "No AI, nothing leaves the PC",
    svg: () => owl({ eyes: "closed", wingL: [66, 166], wingR: [174, 166] }) +
      `<text x="176" y="50" font-family="Segoe UI, Arial, sans-serif" font-size="22" font-weight="800" fill="${C.bodyLight}">z</text>
       <text x="194" y="30" font-family="Segoe UI, Arial, sans-serif" font-size="30" font-weight="800" fill="${C.bodyLight}">Z</text>`,
  },
};

const file = (inner) => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 210" width="240" height="210" role="img">${defs}${inner}</svg>\n`;
for (const [name, pose] of Object.entries(POSES)) writeFileSync(join(OUT, `nox-${name}.svg`), file(`<title>Nox, ${pose.title}</title>` + pose.svg()));

const names = Object.keys(POSES);
const cols = 5, cw = 260, ch = 282, rows = Math.ceil(names.length / cols);
const cells = names.map((name, i) => {
  const x = (i % cols) * cw + 10, y = Math.floor(i / cols) * ch + 70, p = POSES[name];
  return `<g transform="translate(${x} ${y})"><rect width="${cw - 20}" height="${ch - 20}" rx="22" fill="#fff" stroke="#E4E4E0"/>
    <g transform="translate(0 6)">${p.svg()}</g>
    <text x="${(cw - 20) / 2}" y="234" text-anchor="middle" font-family="Segoe UI, Arial, sans-serif" font-size="17" font-weight="700" fill="#1B1B1A">${p.title}</text>
    <text x="${(cw - 20) / 2}" y="252" text-anchor="middle" font-family="Segoe UI, Arial, sans-serif" font-size="12" fill="#6B6B66">${p.note}</text></g>`;
}).join("");
const W = cols * cw, H = rows * ch + 80;
writeFileSync(join(OUT, "nox-poses.svg"), `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" width="${W}" height="${H}">${defs}<rect width="100%" height="100%" fill="#F3F3F1"/>
  <text x="24" y="44" font-family="Segoe UI, Arial, sans-serif" font-size="28" font-weight="800" fill="#1B1B1A">Nox</text>
  <text x="90" y="44" font-family="Segoe UI, Arial, sans-serif" font-size="16" fill="#6B6B66">PC Agent's detective owl, one pose per feature</text>${cells}</svg>\n`);
console.log(`wrote ${names.length} poses`);
