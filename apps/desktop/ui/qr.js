// A small QR Code encoder (ISO 18004): byte mode, error correction M, versions 1 to 5, the eight masks
// scored by penalty. Enough for an address (62 characters fit version 4). Inline, no CDN.

const EC_M = {
  // [error codewords per block, blocks, data codewords]
  1: [10, 1, 16],
  2: [16, 1, 28],
  3: [26, 1, 44],
  4: [18, 2, 64],
  5: [24, 2, 86],
};
const ALIGN = { 1: [], 2: [6, 18], 3: [6, 22], 4: [6, 26], 5: [6, 30] };

const EXP = new Array(512);
const LOG = new Array(256);
(() => {
  let x = 1;
  for (let i = 0; i < 255; i++) {
    EXP[i] = x;
    LOG[x] = i;
    x <<= 1;
    if (x & 0x100) x ^= 0x11d;
  }
  for (let i = 255; i < 512; i++) EXP[i] = EXP[i - 255];
})();

function gfMul(a, b) {
  if (a === 0 || b === 0) return 0;
  return EXP[LOG[a] + LOG[b]];
}

// Generator polynomial coefficients for `degree` error codewords (monic term implied).
function rsDivisor(degree) {
  const result = new Array(degree).fill(0);
  result[degree - 1] = 1;
  let root = 1;
  for (let i = 0; i < degree; i++) {
    for (let j = 0; j < degree; j++) {
      result[j] = gfMul(result[j], root);
      if (j + 1 < degree) result[j] ^= result[j + 1];
    }
    root = gfMul(root, 2);
  }
  return result;
}

function rsRemainder(data, divisor) {
  const res = new Array(divisor.length).fill(0);
  for (const b of data) {
    const factor = b ^ res.shift();
    res.push(0);
    divisor.forEach((d, i) => { res[i] ^= gfMul(d, factor); });
  }
  return res;
}

// Format bits for error level M (00) and a mask, BCH(15,5) and XOR-masked.
function formatBits(mask) {
  const data = mask; // level M is 00
  let rem = data;
  for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
  return ((data << 10) | rem) ^ 0x5412;
}

function bitsOf(value, count) {
  const out = [];
  for (let i = count - 1; i >= 0; i--) out.push((value >>> i) & 1);
  return out;
}

function chooseVersion(length) {
  for (let v = 1; v <= 5; v++) {
    if (12 + 8 * length <= 8 * EC_M[v][2]) return v;
  }
  throw new RangeError('Text is too long for this QR encoder');
}

function dataCodewords(bytes, version) {
  const capacity = EC_M[version][2];
  const bits = [...bitsOf(0b0100, 4), ...bitsOf(bytes.length, 8)];
  for (const b of bytes) bits.push(...bitsOf(b, 8));
  const total = capacity * 8;
  bits.push(...new Array(Math.min(4, total - bits.length)).fill(0));
  while (bits.length % 8) bits.push(0);
  const out = [];
  for (let i = 0; i < bits.length; i += 8) out.push(bits.slice(i, i + 8).reduce((a, b) => a * 2 + b, 0));
  for (let pad = 0xec; out.length < capacity; pad = pad === 0xec ? 0x11 : 0xec) out.push(pad);
  return out;
}

function interleave(data, version) {
  const [ecLen, blockCount, dataLen] = EC_M[version];
  const blockSize = dataLen / blockCount;
  const blocks = [];
  const divisor = rsDivisor(ecLen);
  for (let b = 0; b < blockCount; b++) {
    const d = data.slice(b * blockSize, (b + 1) * blockSize);
    blocks.push({ d, e: rsRemainder(d, divisor) });
  }
  const out = [];
  for (let i = 0; i < blockSize; i++) for (const bl of blocks) out.push(bl.d[i]);
  for (let i = 0; i < ecLen; i++) for (const bl of blocks) out.push(bl.e[i]);
  return out;
}

// Builds the module grid: true is dark. `func` marks function modules so data and masks skip them.
function drawFunctionPatterns(version) {
  const size = 17 + 4 * version;
  const mod = Array.from({ length: size }, () => new Array(size).fill(false));
  const func = Array.from({ length: size }, () => new Array(size).fill(false));
  const set = (x, y, dark) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    mod[y][x] = dark;
    func[y][x] = true;
  };
  for (let i = 0; i < size; i++) {
    set(6, i, i % 2 === 0);
    set(i, 6, i % 2 === 0);
  }
  for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]]) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const dist = Math.max(Math.abs(dx), Math.abs(dy));
        set(cx + dx, cy + dy, dist !== 2 && dist !== 4 && dist <= 3);
      }
    }
  }
  const pos = ALIGN[version];
  for (const cy of pos) {
    for (const cx of pos) {
      const corner = (cx === 6 && cy === 6) || (cx === 6 && cy === size - 7) || (cx === size - 7 && cy === 6);
      if (corner) continue;
      for (let dy = -2; dy <= 2; dy++) {
        for (let dx = -2; dx <= 2; dx++) set(cx + dx, cy + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
      }
    }
  }
  // Format areas are reserved here and written per mask later.
  drawFormat(mod, func, 0);
  return { mod, func, size };
}

function drawFormat(mod, func, mask) {
  const size = mod.length;
  const bits = formatBits(mask);
  const bit = (i) => ((bits >>> i) & 1) === 1;
  const put = (x, y, dark) => {
    mod[y][x] = dark;
    func[y][x] = true;
  };
  for (let i = 0; i <= 5; i++) put(8, i, bit(i));
  put(8, 7, bit(6));
  put(8, 8, bit(7));
  put(7, 8, bit(8));
  for (let i = 9; i < 15; i++) put(14 - i, 8, bit(i));
  for (let i = 0; i < 8; i++) put(size - 1 - i, 8, bit(i));
  for (let i = 8; i < 15; i++) put(8, size - 15 + i, bit(i));
  put(8, size - 8, true);
}

function placeData(mod, func, bits) {
  const size = mod.length;
  let i = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const upward = ((right + 1) & 2) === 0;
        const y = upward ? size - 1 - vert : vert;
        if (!func[y][x] && i < bits.length) {
          mod[y][x] = bits[i] === 1;
          i++;
        }
      }
    }
  }
}

const MASKS = [
  (x, y) => (x + y) % 2 === 0,
  (x, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(y / 2) + Math.floor(x / 3)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

function penalty(mod) {
  const n = mod.length;
  let score = 0;
  const runs = (get) => {
    let total = 0;
    for (let a = 0; a < n; a++) {
      let run = 1;
      for (let b = 1; b < n; b++) {
        if (get(a, b) === get(a, b - 1)) {
          run++;
        } else {
          if (run >= 5) total += run - 2;
          run = 1;
        }
      }
      if (run >= 5) total += run - 2;
    }
    return total;
  };
  score += runs((a, b) => mod[a][b]);
  score += runs((a, b) => mod[b][a]);
  for (let y = 0; y < n - 1; y++) {
    for (let x = 0; x < n - 1; x++) {
      const c = mod[y][x];
      if (c === mod[y][x + 1] && c === mod[y + 1][x] && c === mod[y + 1][x + 1]) score += 3;
    }
  }
  const A = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
  const B = [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1];
  const matchAt = (line, from, pat) => pat.every((v, k) => (line[from + k] ? 1 : 0) === v);
  for (let a = 0; a < n; a++) {
    const row = mod[a];
    const col = mod.map((r) => r[a]);
    for (let b = 0; b + 11 <= n; b++) {
      if (matchAt(row, b, A) || matchAt(row, b, B)) score += 40;
      if (matchAt(col, b, A) || matchAt(col, b, B)) score += 40;
    }
  }
  let dark = 0;
  for (const row of mod) for (const c of row) if (c) dark++;
  const k = Math.floor((Math.abs(dark * 20 - n * n * 10) + n * n - 1) / (n * n)) - 1;
  score += 10 * k;
  return score;
}

// The QR matrix for `text` as rows of booleans (true is dark), quiet zone not included.
export function qrMatrix(text) {
  const bytes = [...new TextEncoder().encode(text)];
  const version = chooseVersion(bytes.length);
  const codewords = interleave(dataCodewords(bytes, version), version);
  const bits = codewords.flatMap((c) => bitsOf(c, 8));
  const base = drawFunctionPatterns(version);
  placeData(base.mod, base.func, bits);

  let best = null;
  let bestScore = Infinity;
  MASKS.forEach((maskFn, mask) => {
    const mod = base.mod.map((row, y) => row.map((c, x) => (base.func[y][x] ? c : c !== maskFn(x, y))));
    drawFormat(mod, base.func, mask);
    const s = penalty(mod);
    if (s < bestScore) {
      bestScore = s;
      best = mod;
    }
  });
  return best;
}

// An inline SVG element holding the QR code with a quiet zone, for the page.
export function qrSvg(text, { quiet = 4, dark = 'currentColor' } = {}) {
  const m = qrMatrix(text);
  const n = m.length + quiet * 2;
  const NS = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('viewBox', `0 0 ${n} ${n}`);
  svg.setAttribute('role', 'img');
  svg.setAttribute('shape-rendering', 'crispEdges');
  const path = document.createElementNS(NS, 'path');
  let d = '';
  m.forEach((row, y) => row.forEach((c, x) => { if (c) d += `M${x + quiet} ${y + quiet}h1v1h-1z`; }));
  path.setAttribute('d', d);
  path.setAttribute('fill', dark);
  svg.append(path);
  return svg;
}
