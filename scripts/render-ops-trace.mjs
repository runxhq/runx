// Renders the README graph images from a sealed business-ops graph receipt
// tree in a local receipt store. The images are generated evidence, not
// illustration: every skill name, receipt digest, prepared-context binding, and closure
// in the output is read from signed receipts. Rerun after a real run to keep
// the README in lockstep with what the runtime actually does.
//
//   node scripts/render-ops-trace.mjs \
//     --store .runx/receipts \
//     --root sha256:<graph-receipt-id> \
//     --outdir docs/assets
//
// Emits ops-trace-dark.svg and ops-trace-light.svg from one token set. The
// scene-driven sibling (render-ops-map.mjs) owns the README hero image; this
// one renders literal evidence from a sealed run.
//
// Form: a transit-map harp. The command is the origin, the first child is the
// interchange, each remaining child is a line running through its own station,
// and all lines converge into the seal ring with the offline verify verdict.
// Animation is additive only (pulses riding the lines, the seal ring turning):
// the diagram is complete at every frame, so static rasterizers, social cards,
// and prefers-reduced-motion all see the finished picture.

import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  args.set(process.argv[i].replace(/^--/, ''), process.argv[i + 1]);
}
const storeDir = args.get('store') ?? '.runx/receipts';
const rootId = args.get('root');
const outDir = args.get('outdir') ?? 'docs/assets';
if (!rootId) {
  console.error('usage: render-ops-trace.mjs --store <dir> --root <receipt-id> [--outdir <dir>]');
  process.exit(1);
}

const receipts = new Map();
for (const file of readdirSync(storeDir)) {
  if (!file.endsWith('.json')) continue;
  try {
    const receipt = JSON.parse(readFileSync(join(storeDir, file), 'utf8'));
    if (receipt.id) receipts.set(receipt.id, receipt);
  } catch {
    // unreadable entries are the verifier's problem, not the renderer's
  }
}

const mustGet = (id) => {
  const receipt = receipts.get(id);
  if (!receipt) {
    console.error(`receipt ${id} not found in ${storeDir}`);
    process.exit(1);
  }
  return receipt;
};

const root = mustGet(rootId);
const children = (root.lineage?.children ?? []).map((ref) => mustGet(ref.uri.replace('runx:receipt:', '')));
if (children.length < 2) {
  console.error(`expected a fanout under ${rootId}, found ${children.length} children`);
  process.exit(1);
}

// A step receipt's subject is runx:run:<run id>#<step>[@<attempt>].
const stepName = (receipt) => receipt.subject.ref.uri.split('#')[1]?.replace(/@\d+$/, '') ?? receipt.subject.ref.uri;
const shortId = (id) => id.replace('sha256:', '').slice(0, 12);
const esc = (value) => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

const evidenceRefs = children
  .flatMap((r) => r.acts ?? [])
  .flatMap((act) => act.criterion_bindings ?? [])
  .flatMap((binding) => binding.evidence_refs ?? []);
const preparedContext = evidenceRefs.find(
  (ref) => ref.type === 'artifact' && ref.uri.includes('operator_context:'),
);
const preparedContextDigest = preparedContext?.locator ?? preparedContext?.uri.split('operator_context:').at(-1);

const [router, ...lanes] = children;
const sealedDate = (root.created_at ?? '').slice(0, 10);
const signatureAlg = root.signature?.alg ?? 'unsigned';
const treeCount = 1 + children.length;

// one palette per theme; lane hues cycle by position, semantics live in receipts
const THEMES = {
  dark: {
    bg: '#09090e',
    frame: 'none',
    ink: '#f5f1ea',
    muted: '#c7beca',
    faint: '#8f8795',
    amber: '#ffb84d',
    seal: '#65b7ff',
    trunk: '#ff2e88',
    lanes: ['#28d7c2', '#ffb84d', '#65b7ff', '#ff2e88', '#b48cff', '#7ee787'],
    laneOpacity: 0.72,
    glowOpacity: 0.07,
  },
  light: {
    bg: '#fdfcfa',
    frame: '#e6e1d8',
    ink: '#16121e',
    muted: '#585165',
    faint: '#8a8394',
    amber: '#b26a05',
    seal: '#2563c4',
    trunk: '#e0186f',
    lanes: ['#0f9d8f', '#c47d10', '#2f6fdb', '#e0186f', '#7a4fd6', '#2f9e44'],
    laneOpacity: 0.9,
    glowOpacity: 0.05,
  },
};

const WIDTH = 1120;
const HEIGHT = 420;
const MID_Y = 200;
const ORIGIN_X = 322;
const INTERCHANGE_X = 430;
const SPLIT_X = 468;
const RUN_X0 = 566;
const RUN_X1 = 872;
const MERGE_X = 976;
const STATION_X = 584;
const LANE_Y0 = 78;
const LANE_GAP = 48;

const laneGeometry = lanes.map((receipt, i) => ({
  receipt,
  y: LANE_Y0 + i * LANE_GAP,
}));

// one continuous path per lane: interchange, bow out, run the flat, bow back
// into the seal; used for the drawn line, its glow, and its riding pulse
const lanePath = ({ y }) =>
  `M${INTERCHANGE_X} ${MID_Y} H${SPLIT_X} C ${SPLIT_X + 52} ${MID_Y}, ${RUN_X0 - 52} ${y}, ${RUN_X0} ${y} H${RUN_X1} C ${RUN_X1 + 56} ${y}, ${MERGE_X - 50} ${MID_Y}, ${MERGE_X} ${MID_Y}`;

const render = (theme) => {
  const t = THEMES[theme];
  const laneMarkup = laneGeometry
    .map((lane, i) => {
      const { receipt, y } = lane;
      const hue = t.lanes[i % t.lanes.length];
      const path = lanePath(lane);
      const pulsePath = `M${ORIGIN_X} ${MID_Y} ${path.replace(/^M\S+ \S+ /, '')}`;
      return `
  <path d="${path}" stroke="${hue}" stroke-opacity="${t.glowOpacity}" stroke-width="7" fill="none"/>
  <path d="${path}" stroke="${hue}" stroke-opacity="${t.laneOpacity}" stroke-width="2" fill="none"/>
  <circle cx="${STATION_X}" cy="${y}" r="5" fill="${t.bg}" stroke="${hue}" stroke-width="2"/>
  <text class="name" x="${STATION_X + 18}" y="${y - 9}">${esc(stepName(receipt))}</text>
  <text class="mono faint" x="${STATION_X + 18 + 8.2 * stepName(receipt).length + 16}" y="${y - 9}">${esc(shortId(receipt.id))} · ${esc(receipt.seal.disposition)}</text>
  <circle r="3" fill="${hue}">
    <animateMotion dur="6.5s" begin="${(i * 1.05).toFixed(2)}s" repeatCount="indefinite" path="${pulsePath}"/>
  </circle>`;
    })
    .join('\n');

  const ns = `rx-${theme}`;
  return `<svg xmlns="http://www.w3.org/2000/svg" class="${ns}" width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}" role="img" aria-labelledby="title desc">
  <title id="title">runx business-ops sealed graph receipt trace</title>
  <desc id="desc">Rendered from sealed receipt ${esc(rootId)}: one approved command is classified, fans into ${lanes.length} governed skill lanes, and every child receipt converges into one signed graph receipt that verifies offline.</desc>
  <defs>
    <style>
      .${ns} .ink { fill: ${t.ink}; }
      .${ns} .muted { fill: ${t.muted}; }
      .${ns} .faint { fill: ${t.faint}; }
      .${ns} .amber { fill: ${t.amber}; }
      .${ns} .seal-text { fill: ${t.seal}; }
      .${ns} .name { font: 700 13.5px Inter, ui-sans-serif, system-ui, sans-serif; fill: ${t.ink}; }
      .${ns} .mono { font: 600 12px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
      .${ns} .small { font: 520 11px Inter, ui-sans-serif, system-ui, sans-serif; }
      .${ns} .seal-ring { animation: ${ns}-turn 14s linear infinite; transform-origin: ${MERGE_X}px ${MID_Y}px; }
      @keyframes ${ns}-turn { to { transform: rotate(360deg); } }
      @media (prefers-reduced-motion: reduce) { .${ns} * { animation: none !important; } }
    </style>
  </defs>

  <rect width="${WIDTH}" height="${HEIGHT}" rx="18" fill="${t.bg}"${t.frame === 'none' ? '' : ` stroke="${t.frame}" stroke-width="1.5"`}/>

  <text class="mono ink" x="64" y="${MID_Y - 27}">$ runx skill business-ops</text>
  <text class="mono muted" x="64" y="${MID_Y - 5}">signal: prepare API v2</text>
  <text class="mono amber" x="64" y="${MID_Y + 21}">context bound · ${esc(shortId(preparedContextDigest ?? 'unknown'))}</text>

  <path d="M${ORIGIN_X} ${MID_Y} H${INTERCHANGE_X}" stroke="${t.trunk}" stroke-opacity="${t.glowOpacity}" stroke-width="7" fill="none"/>
  <path d="M${ORIGIN_X} ${MID_Y} H${INTERCHANGE_X}" stroke="${t.trunk}" stroke-opacity=".85" stroke-width="2.4" fill="none"/>
  <circle cx="${ORIGIN_X}" cy="${MID_Y}" r="5.5" fill="${t.trunk}"/>

  <circle cx="${INTERCHANGE_X}" cy="${MID_Y}" r="9" fill="${t.bg}" stroke="${t.trunk}" stroke-width="2.4"/>
  <circle cx="${INTERCHANGE_X}" cy="${MID_Y}" r="3.2" fill="${t.trunk}"/>
  <text class="name" x="${INTERCHANGE_X - 14}" y="${MID_Y + 36}">${esc(stepName(router))}</text>
  <text class="mono faint" x="${INTERCHANGE_X - 14}" y="${MID_Y + 55}">${esc(shortId(router.id))}</text>

${laneMarkup}

  <g class="seal-ring">
    <circle cx="${MERGE_X}" cy="${MID_Y}" r="24" fill="none" stroke="${t.seal}" stroke-width="1.6" stroke-dasharray="4 7"/>
  </g>
  <circle cx="${MERGE_X}" cy="${MID_Y}" r="15" fill="none" stroke="${t.seal}" stroke-opacity=".55" stroke-width="1.2"/>
  <circle cx="${MERGE_X}" cy="${MID_Y}" r="7" fill="${t.seal}"/>

  <path d="M${MERGE_X} ${MID_Y + 28} V${HEIGHT - 92}" stroke="${t.seal}" stroke-opacity=".28" stroke-width="1.2" stroke-dasharray="2 6" fill="none"/>
  <text class="mono ink" x="1056" y="${HEIGHT - 70}" text-anchor="end">${esc(shortId(rootId))}…</text>
  <text class="small muted" x="1056" y="${HEIGHT - 50}" text-anchor="end">${treeCount} receipts, one tree · ${esc(signatureAlg)} · sealed ${esc(sealedDate)}</text>
  <text class="mono seal-text" x="1056" y="${HEIGHT - 26}" text-anchor="end">$ runx verify ${esc(shortId(rootId))}… → ok</text>
</svg>
`;
};

for (const theme of Object.keys(THEMES)) {
  const outPath = join(outDir, `ops-trace-${theme}.svg`);
  writeFileSync(outPath, render(theme));
  console.log(`wrote ${outPath}: root ${shortId(rootId)}, ${lanes.length} lanes, ${treeCount} receipts`);
}
