// data.jsx — icons, node content templates, providers, suggestions

/* ---------- Icon set (stroke, lucide-style) ---------- */
const ICON_PATHS = {
  shield: "M12 3l7 3v5c0 4.5-3 8.2-7 9.5C8 19.2 5 15.5 5 11V6l7-3z",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z",
  compass: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M15.5 8.5l-2 5-5 2 2-5 5-2z",
  grid: "M4 4h7v7H4V4z M13 4h7v7h-7V4z M4 13h7v7H4v-7z M13 13h7v7h-7v-7z",
  plug: "M9 7V3 M15 7V3 M6 7h12v4a6 6 0 0 1-12 0V7z M12 17v4",
  box: "M21 8l-9-5-9 5v8l9 5 9-5V8z M3 8l9 5 9-5 M12 13v8",
  palette: "M12 21a9 9 0 1 1 9-9c0 2-1.5 3-3 3h-2a2 2 0 0 0-2 2c0 1 .5 1.5.5 2.5S13.5 21 12 21z M7.5 11.5h.01 M10.5 7.5h.01 M14.5 7.5h.01 M17 11h.01",
  gear: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M19.4 15a1.6 1.6 0 0 0 .3 1.7l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-1.7-.3 1.6 1.6 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.2a1.6 1.6 0 0 0-1-1.5 1.6 1.6 0 0 0-1.7.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0 .3-1.7 1.6 1.6 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.2a1.6 1.6 0 0 0 1.5-1 1.6 1.6 0 0 0-.3-1.7l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 1.7.3h.1a1.6 1.6 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.2a1.6 1.6 0 0 0 1 1.5h.1a1.6 1.6 0 0 0 1.7-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0-.3 1.7v.1a1.6 1.6 0 0 0 1.5 1h.2a2 2 0 1 1 0 4h-.2a1.6 1.6 0 0 0-1.5 1z",
  sparkles: "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9L12 3z M19 15l.9 2.1L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.9L19 15z",
  wand: "M15 4V2 M15 10V8 M11.5 6.5h-2 M20.5 6.5h-2 M17.8 3.7l-1.4 1.4 M13.6 7.9l-1.4 1.4 M17.8 9.3l-1.4-1.4 M12 10l-9 9 2 2 9-9-2-2z",
  paperclip: "M21 12.5l-8.2 8.2a5.5 5.5 0 0 1-7.8-7.8l8.5-8.5a3.7 3.7 0 0 1 5.2 5.2l-8.5 8.5a1.8 1.8 0 0 1-2.6-2.6l7.8-7.8",
  send: "M5 12L3.3 4.3a.4.4 0 0 1 .6-.5l16.6 7.8a.4.4 0 0 1 0 .8L3.9 20.2a.4.4 0 0 1-.6-.5L5 12z M5 12h7",
  layers: "M12 3l9 5-9 5-9-5 9-5z M4.5 12.8L12 17l7.5-4.2 M4.5 16.8L12 21l7.5-4.2",
  chevDown: "M6 9l6 6 6-6",
  chevRight: "M9 6l6 6-6 6",
  check: "M4.5 12.5l5 5 10-11",
  x: "M6 6l12 12 M18 6L6 18",
  plus: "M12 5v14 M5 12h14",
  minus: "M5 12h14",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M21 21l-5-5",
  bell: "M18 9a6 6 0 1 0-12 0c0 6-2.5 7-2.5 7h17S18 15 18 9z M10.3 20a2 2 0 0 0 3.4 0",
  lock: "M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1z M8 11V7a4 4 0 0 1 8 0v4",
  code: "M8 7l-5 5 5 5 M16 7l5 5-5 5",
  save: "M5 3h11l3 3v15H5V3z M8 3v5h7V3 M8 14h8v7H8v-7z",
  fit: "M4 9V4h5 M20 9V4h-5 M4 15v5h5 M20 15v5h-5",
  front: "M8 8h12v12H8V8z M4 16V4h12",
  toBack: "M4 4h12v12H4V4z M20 8v12H8",
  up: "M12 19V5 M5 12l7-7 7 7",
  down: "M12 5v14 M5 12l7 7 7-7",
  type: "M5 7V5h14v2 M12 5v14 M9 19h6",
  expand: "M9 3H3v6 M15 3h6v6 M21 15v6h-6 M3 15v6h6",
  collapse: "M3 9h6V3 M21 9h-6V3 M15 21v-6h6 M9 21v-6H3",
  cursor: "M4 4l7 16 2.5-6.5L20 11 4 4z",
  marquee: "M9 4H4v5 M15 4h5v5 M20 15v5h-5 M4 15v5h5 M12 10.5l.9 2.6 2.6.9-2.6.9-.9 2.6-.9-2.6-2.6-.9 2.6-.9.9-2.6z",
  copy: "M9 9h11v11H9V9z M5 15H4V4h11v1",
  trash: "M4 7h16 M9 7V4h6v3 M6 7l1 14h10l1-14 M10 11v6 M14 11v6",
  image: "M4 5h16a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z M8.5 11a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3z M21 15l-5-5-9 9",
  file: "M6 2h8l5 5v15H6V2z M14 2v5h5",
  clock: "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M12 7v5l3 2",
  gauge: "M12 15a2 2 0 1 0 0-4 2 2 0 0 0 0 4z M13.4 10.6L17 7 M5 17a9 9 0 1 1 14 0",
  loader: "M12 3v3 M12 18v3 M3 12h3 M18 12h3 M5.6 5.6l2.1 2.1 M16.3 16.3l2.1 2.1 M5.6 18.4l2.1-2.1 M16.3 7.7l2.1-2.1",
  terminal: "M4 17l5-5-5-5 M11 19h9",
  cpu: "M8 8h8v8H8V8z M5 5h14v14H5V5z M9 2v3 M15 2v3 M9 19v3 M15 19v3 M2 9h3 M2 15h3 M19 9h3 M19 15h3",
  cloud: "M7 18a4.5 4.5 0 1 1 .8-8.9A6 6 0 0 1 19 10a4 4 0 0 1-1 7.9L7 18z",
  undo: "M8 7l-5 5 5 5 M3 12h12a5 5 0 0 1 0 10h-4",
  redo: "M16 7l5 5-5 5 M21 12H9a5 5 0 0 0 0 10h4",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7-10-7-10-7z M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  eyeOff: "M3 3l18 18 M10.6 10.6a3 3 0 0 0 4.2 4.2 M9.8 5.1A10.7 10.7 0 0 1 12 5c6 0 10 7 10 7a18.5 18.5 0 0 1-3.1 3.8 M6.5 6.6A18.3 18.3 0 0 0 2 12s4 7 10 7a10.7 10.7 0 0 0 3.3-.5",
  alert: "M12 3l9.5 17H2.5L12 3z M12 10v4 M12 17h.01",
  branch: "M6 4a2 2 0 1 0 0 4 2 2 0 0 0 0-4z M6 8v8 M6 16a2 2 0 1 0 0 4 2 2 0 0 0 0-4z M18 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4z M18 10c0 4-4 4-6 6",
};

function Icon({ name, size = 18, sw = 1.7, style }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth={sw} strokeLinecap="round" strokeLinejoin="round" style={style} aria-hidden="true">
      {ICON_PATHS[name].split(" M").map((d, i) => (
        <path key={i} d={(i === 0 ? "" : "M") + d} />
      ))}
    </svg>
  );
}

/* ---------- Providers ---------- */
const DESIGN_PROVIDERS = [
  { id: "claude", name: "Claude Code", desc: "CLI agent · grounds via MCP", badge: "MCP", icon: "terminal" },
  { id: "codex", name: "Codex", desc: "CLI agent · grounds via MCP", badge: "MCP", icon: "terminal" },
  { id: "ollama", name: "Ollama", desc: "Local HTTP · streams live", badge: "LOCAL", icon: "cpu" },
  { id: "omlx", name: "oMLX", desc: "Local HTTP · streams live", badge: "LOCAL", icon: "cpu" },
];
const EFFORT_LEVELS = ["Low", "Medium", "High"];

/* ---------- Agent handoff: local agents that wire up backend & config ---------- */
const AGENT_TASKS = [
  { agent: "Scaffold", icon: "box",  label: "Scaffold routes & handlers",   detail: "4 sections → 6 endpoints" },
  { agent: "Data",     icon: "grid", label: "Wire data models & queries",   detail: "schema · 3 models" },
  { agent: "Bind",     icon: "code", label: "Bind components to live data",  detail: "props → fetch hooks" },
  { agent: "Config",   icon: "gear", label: "Configure env & secrets",      detail: ".env · provider keys" },
  { agent: "QA",       icon: "check",label: "Typecheck & smoke test",       detail: "tsc · 0 errors" },
];

const PROJECTS = [
  { id: "demo", name: "Demo landing", meta: "4 nodes · updated today", color: "#EFD9BC" },
  { id: "pricing", name: "Pricing revamp", meta: "7 nodes · 2 days ago", color: "#DCE5D2" },
  { id: "onboard", name: "Onboarding email", meta: "3 nodes · last week", color: "#E3D7E8" },
];

const SUGGESTIONS = [
  { icon: "sparkles", text: "A pricing section coherent with our app" },
  { icon: "image", text: "Hero with a product screenshot placeholder" },
  { icon: "wand", text: "Redesign the CTA using our brand accent" },
];

const ORACLE_SOURCES = [
  "src/components/Pricing.tsx",
  "src/styles/tokens.css",
  "tailwind.config.js",
];

/* ---------- Node content (sanitized HTML, internal layout only) ---------- */
const NF = `font-family:'Source Serif 4',Georgia,serif`;
const NS = `font-family:'Instrument Sans',system-ui,sans-serif`;

const NODE_HTML = {
  hero: `
  <div style="${NS};padding:46px 44px;background:linear-gradient(160deg,#FFF9F0,#FBEEDD)">
    <div style="display:inline-flex;align-items:center;gap:7px;font-size:11px;font-weight:700;letter-spacing:.1em;color:#9A4A1C;background:#fff;border:1px solid #EBD9C2;border-radius:99px;padding:5px 12px;margin-bottom:20px">DEVBOULE</div>
    <div style="${NF};font-size:42px;line-height:1.08;letter-spacing:-.02em;color:#37291A;font-weight:600;max-width:430px">Build in lockstep with your codebase</div>
    <p style="margin:16px 0 26px;font-size:16px;line-height:1.55;color:#7A6B56;max-width:400px">Design that stays coherent with the product being built — its components, palette and functionality.</p>
    <div style="display:flex;gap:12px;align-items:center">
      <span style="background:#C14B1B;color:#fff;font-weight:600;font-size:14.5px;padding:12px 22px;border-radius:10px">Get started</span>
      <span style="color:#6B5A44;font-weight:600;font-size:14.5px;padding:12px 18px;border-radius:10px;border:1px solid #E4D3BC;background:#fff">See the docs</span>
    </div>
  </div>`,

  features: `
  <div style="${NS};padding:34px 32px;background:#fff">
    <div style="font-size:11px;font-weight:700;letter-spacing:.12em;color:#A08B6B;margin-bottom:18px">WHY DEVBOULE</div>
    <div style="display:grid;grid-template-columns:1fr 1fr 1fr;gap:14px">
      <div style="background:#FBF6EE;border:1px solid #EFE3D0;border-radius:13px;padding:18px">
        <div style="width:30px;height:30px;border-radius:8px;background:#F3E2CC;margin-bottom:12px"></div>
        <div style="font-weight:650;font-size:14.5px;color:#3B2F20">Grounded</div>
        <div style="font-size:12.5px;line-height:1.5;color:#8A7A62;margin-top:5px">Reads your real components and tokens.</div>
      </div>
      <div style="background:#FBF6EE;border:1px solid #EFE3D0;border-radius:13px;padding:18px">
        <div style="width:30px;height:30px;border-radius:8px;background:#E5E9DB;margin-bottom:12px"></div>
        <div style="font-weight:650;font-size:14.5px;color:#3B2F20">Deterministic</div>
        <div style="font-size:12.5px;line-height:1.5;color:#8A7A62;margin-top:5px">Placement is geometry, never a guess.</div>
      </div>
      <div style="background:#FBF6EE;border:1px solid #EFE3D0;border-radius:13px;padding:18px">
        <div style="width:30px;height:30px;border-radius:8px;background:#EBDDE5;margin-bottom:12px"></div>
        <div style="font-weight:650;font-size:14.5px;color:#3B2F20">Two-way</div>
        <div style="font-size:12.5px;line-height:1.5;color:#8A7A62;margin-top:5px">Design to code, code back to design.</div>
      </div>
    </div>
  </div>`,

  cta: `
  <div style="${NS};padding:30px 32px;background:#3B2D1D;border-radius:0">
    <div style="${NF};font-size:24px;color:#F8EFDF;letter-spacing:-.01em">Ready when you are.</div>
    <div style="display:flex;align-items:center;justify-content:space-between;margin-top:14px;gap:14px">
      <div style="font-size:13.5px;color:#C9B797;line-height:1.5">Open a working folder and start designing.</div>
      <span style="flex:none;background:#F3E3CB;color:#4A3013;font-weight:650;font-size:14px;padding:11px 20px;border-radius:10px">Open folder</span>
    </div>
  </div>`,

  ctaAlt: `
  <div style="${NS};padding:30px 32px;background:#3B2D1D;border-radius:0">
    <div style="${NF};font-size:24px;color:#F8EFDF;letter-spacing:-.01em">Ready when you are.</div>
    <div style="display:flex;align-items:center;justify-content:space-between;margin-top:14px;gap:14px">
      <div style="font-size:13.5px;color:#C9B797;line-height:1.5">Open a working folder and start designing.</div>
      <span style="flex:none;background:#C14B1B;color:#fff;font-weight:650;font-size:14px;padding:11px 20px;border-radius:10px;box-shadow:0 2px 8px rgba(193,75,27,.4)">Open folder</span>
    </div>
  </div>`,

  pricing: `
  <div style="${NS};padding:36px 34px;background:#fff">
    <div style="${NF};font-size:27px;color:#37291A;letter-spacing:-.015em;text-align:center">Simple pricing</div>
    <div style="font-size:13.5px;color:#8A7A62;text-align:center;margin:7px 0 24px">Per seat, billed monthly. Cancel anytime.</div>
    <div style="display:grid;grid-template-columns:1fr 1fr;gap:14px">
      <div style="border:1px solid #EFE3D0;border-radius:14px;padding:22px">
        <div style="font-weight:650;font-size:14px;color:#3B2F20">Lab</div>
        <div style="margin:10px 0 4px"><span style="${NF};font-size:32px;color:#37291A">$29</span><span style="font-size:12.5px;color:#9A8A70"> /seat</span></div>
        <div style="font-size:12.5px;color:#8A7A62;line-height:1.7;margin:10px 0 16px">Oracle grounding<br>3 working folders<br>Export to code</div>
        <span style="display:block;text-align:center;border:1px solid #E4D3BC;color:#6B5A44;font-weight:600;font-size:13px;padding:10px;border-radius:9px">Choose Lab</span>
      </div>
      <div style="border:2px solid #C14B1B;border-radius:14px;padding:22px;position:relative;background:#FFFAF4">
        <div style="position:absolute;top:-9px;left:50%;transform:translateX(-50%);background:#C14B1B;color:#fff;font-size:10px;font-weight:700;letter-spacing:.08em;padding:3px 10px;border-radius:99px">POPULAR</div>
        <div style="font-weight:650;font-size:14px;color:#3B2F20">Facility</div>
        <div style="margin:10px 0 4px"><span style="${NF};font-size:32px;color:#37291A">$79</span><span style="font-size:12.5px;color:#9A8A70"> /seat</span></div>
        <div style="font-size:12.5px;color:#8A7A62;line-height:1.7;margin:10px 0 16px">Everything in Lab<br>Unlimited folders<br>Design-token sync</div>
        <span style="display:block;text-align:center;background:#C14B1B;color:#fff;font-weight:600;font-size:13px;padding:10px;border-radius:9px">Choose Facility</span>
      </div>
    </div>
  </div>`,

  quotes: `
  <div style="${NS};padding:34px 32px;background:linear-gradient(160deg,#FFF9F0,#FBEEDD)">
    <div style="font-size:11px;font-weight:700;letter-spacing:.12em;color:#A08B6B;margin-bottom:16px">WHAT TEAMS SAY</div>
    <div style="${NF};font-size:21px;line-height:1.4;color:#42321E;font-style:italic">“The designs it produces already use our components. Nothing feels pasted in.”</div>
    <div style="display:flex;align-items:center;gap:11px;margin-top:18px">
      <div style="width:34px;height:34px;border-radius:50%;background:#E2C49C"></div>
      <div><div style="font-weight:650;font-size:13.5px;color:#3B2F20">Mara Voss</div><div style="font-size:12px;color:#9A8A70">Head of Product, Helix Labs</div></div>
    </div>
  </div>`,

  footer: `
  <div style="${NS};padding:26px 32px;background:#fff;border-top:1px solid #EFE3D0">
    <div style="display:flex;align-items:center;justify-content:space-between;gap:16px">
      <div style="display:flex;align-items:center;gap:9px"><div style="width:22px;height:22px;border-radius:50%;background:#E2C49C"></div><span style="font-weight:650;font-size:13.5px;color:#3B2F20">Devboule</span></div>
      <div style="display:flex;gap:18px;font-size:12.5px;color:#8A7A62"><span>Product</span><span>Docs</span><span>Pricing</span><span>Contact</span></div>
      <div style="font-size:12px;color:#B3A48C">© 2026</div>
    </div>
  </div>`,
};

const SKELETON = `
  <div class="skel" style="background:#FBF8F2">
    <i style="height:14px;width:90px"></i>
    <i style="height:30px;width:75%"></i>
    <i style="height:13px;width:88%"></i>
    <i style="height:13px;width:62%"></i>
    <div style="display:flex;gap:12px;margin-top:8px"><i style="height:84px;flex:1"></i><i style="height:84px;flex:1"></i></div>
  </div>`;

/* ---------- Initial canvas state ---------- */
const INITIAL_NODES = [
  { id: "hero", name: "Hero", kind: "html", x: 96, y: 96, z: 1, w: 560, html: NODE_HTML.hero },
  { id: "features", name: "Features", kind: "html", x: 96, y: 470, z: 2, w: 560, html: NODE_HTML.features },
  { id: "cta", name: "CTA", kind: "html", x: 712, y: 96, z: 3, w: 470, html: NODE_HTML.cta },
];

/* Templates cycled through for new generations */
const GEN_TEMPLATES = [
  { match: /pric/i, name: "Pricing", w: 470, html: NODE_HTML.pricing },
  { match: /quote|testimon|say/i, name: "Testimonials", w: 470, html: NODE_HTML.quotes },
  { match: /hero/i, name: "Hero B", w: 560, html: NODE_HTML.hero },
  { match: /.*/, name: "Section", w: 470, html: NODE_HTML.quotes },
];

Object.assign(window, {
  Icon, ICON_PATHS, DESIGN_PROVIDERS, EFFORT_LEVELS, PROJECTS, SUGGESTIONS,
  ORACLE_SOURCES, NODE_HTML, SKELETON, INITIAL_NODES, GEN_TEMPLATES, AGENT_TASKS,
});
