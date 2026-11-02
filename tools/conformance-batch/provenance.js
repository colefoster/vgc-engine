'use strict';
// Capture provenance for conformance-batch outputs.
//
// A battle ID hashes only (format, teams, seed), so an out_<id>.json from an
// older PS checkout or an older conformance-driver.js has the same name as a
// fresh one. run-batch.js stamps `_meta.provenance` on every capture and only
// reuses an existing output whose stamp matches the current oracle exactly.
//
//   ps_revision    PS checkout HEAD (git), or `pokemon-showdown@<version>` for
//                  an npm-installed package. null when neither can be read;
//                  `<sha>-dirty` when tracked files are modified. Neither null
//                  nor dirty revisions are ever reused: they don't pin an oracle.
//   driver_sha256  sha256 of conformance-driver.js (the capture logic).
//   inputs_sha256  sha256 of the job inputs that shape the battle
//                  (format, seed, max_turns, p1 team, p2 team).

const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const { execFileSync } = require('child_process');

const sha256 = (data) => crypto.createHash('sha256').update(data).digest('hex');

function jobInputs(job) {
  return {
    format: job.format || 'gen9customgame',
    seed: job.seed || [1, 2, 3, 4],
    max_turns: job.max_turns || 20,
    p1team: job.p1 && job.p1.team,
    p2team: job.p2 && job.p2.team,
  };
}

// Defaults mirror conformance-driver.js runJob, so a job that omits a field
// digests the same as one that spells out the default.
function inputsDigest(job) {
  const i = jobInputs(job);
  return sha256(JSON.stringify([i.format, i.seed, i.max_turns, i.p1team, i.p2team]));
}

// PS revision for a dist/sim dir: git HEAD of the enclosing checkout, else the
// package.json version of an npm install.
function psRevision(psDist) {
  let dir = path.resolve(psDist);
  for (let i = 0; i < 4 && dir !== path.dirname(dir); i++, dir = path.dirname(dir)) {
    const pkg = path.join(dir, 'package.json');
    if (!fs.existsSync(pkg)) continue;
    let name = null, version = null;
    try { ({ name, version } = JSON.parse(fs.readFileSync(pkg, 'utf8'))); } catch (_) {}
    if (name !== 'pokemon-showdown') continue;
    if (fs.existsSync(path.join(dir, '.git'))) {
      try {
        const git = (...a) => execFileSync('git', ['-C', dir, ...a], { encoding: 'utf8' }).trim();
        const head = git('rev-parse', 'HEAD');
        const dirty = git('status', '--porcelain', '--untracked-files=no') !== '';
        return dirty ? `${head}-dirty` : head;
      } catch (_) {
        return null;
      }
    }
    return version ? `pokemon-showdown@${version}` : null;
  }
  return null;
}

function oracleIdentity({ psDist, driverPath }) {
  return { ps_revision: psRevision(psDist), driver_sha256: sha256(fs.readFileSync(driverPath)) };
}

const provenanceFor = (oracle, job) => ({ ...oracle, inputs_sha256: inputsDigest(job) });

// Pure: may the existing output text be reused under `expected` provenance?
// Returns { reuse: true } or { reuse: false, reason }.
function checkReuse(text, expected) {
  if (!expected.ps_revision) {
    return { reuse: false, reason: 'current PS revision unknown (no git checkout or package version at PS_DIST)' };
  }
  if (expected.ps_revision.endsWith('-dirty')) {
    return { reuse: false, reason: `current PS checkout has local modifications (${expected.ps_revision})` };
  }
  let out;
  try {
    out = JSON.parse(text);
  } catch (e) {
    return { reuse: false, reason: `unreadable or truncated JSON (${e.message})` };
  }
  const got = out && out._meta && out._meta.provenance;
  if (!got) return { reuse: false, reason: 'no _meta.provenance (legacy capture)' };
  for (const [k, label] of [
    ['ps_revision', 'PS revision'],
    ['driver_sha256', 'driver hash'],
    ['inputs_sha256', 'format/team inputs'],
  ]) {
    if (got[k] !== expected[k]) {
      return { reuse: false, reason: `${label} changed: captured ${got[k] || 'none'}, current ${expected[k]}` };
    }
  }
  return { reuse: true };
}

// Move an output aside before it is overwritten: <outDir>/stale/out_<id>.<n>.json.
// Outside the out_*.json glob, so it never feeds the engine by accident.
function preserve(outPath) {
  const dir = path.join(path.dirname(outPath), 'stale');
  fs.mkdirSync(dir, { recursive: true });
  const base = path.basename(outPath, '.json');
  let n = 0;
  let dest;
  do dest = path.join(dir, `${base}.${n++}.json`); while (fs.existsSync(dest));
  fs.renameSync(outPath, dest);
  return dest;
}

module.exports = { inputsDigest, psRevision, oracleIdentity, provenanceFor, checkReuse, preserve };
