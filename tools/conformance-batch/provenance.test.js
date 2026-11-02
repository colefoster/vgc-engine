// Reuse guard for conformance-batch captures (provenance.js, run-batch.js).
// Run:  node --test tools/conformance-batch/provenance.test.js
// Needs git and node only; no PS checkout (run-batch.js gets a stub PS_DIST,
// and every case it runs is reuse-or-stale, so no battle is simulated).

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync, spawnSync } = require('child_process');
const { inputsDigest, psRevision, provenanceFor, checkReuse, preserve } = require('./provenance');

const JOB = {
  id: 'abc', seed: [1, 2, 3, 4], format: 'gen9championsdoublescustomgame', max_turns: 12,
  p1: { team: 'Weavile\n- Crunch' }, p2: { team: 'Garganacl\n- Splash' },
};
const ORACLE = { ps_revision: 'a'.repeat(40), driver_sha256: 'd'.repeat(64) };
const capture = (prov) => JSON.stringify({ id: 'abc', turns: [], _meta: { ok: true, ...(prov && { provenance: prov }) } });

test('same PS revision, driver and inputs: reuse', () => {
  const p = provenanceFor(ORACLE, JOB);
  assert.deepEqual(checkReuse(capture(p), p), { reuse: true });
});

test('changed PS revision: stale', () => {
  const p = provenanceFor(ORACLE, JOB);
  const now = { ...p, ps_revision: 'b'.repeat(40) };
  const v = checkReuse(capture(p), now);
  assert.equal(v.reuse, false);
  assert.match(v.reason, /PS revision changed/);
});

test('changed driver hash: stale', () => {
  const p = provenanceFor(ORACLE, JOB);
  const v = checkReuse(capture(p), { ...p, driver_sha256: 'e'.repeat(64) });
  assert.equal(v.reuse, false);
  assert.match(v.reason, /driver hash changed/);
});

test('changed format/team/seed/max_turns inputs: stale', () => {
  const p = provenanceFor(ORACLE, JOB);
  for (const job of [
    { ...JOB, format: 'gen9customgame' },
    { ...JOB, seed: [9, 9, 9, 9] },
    { ...JOB, max_turns: 20 },
    { ...JOB, p2: { team: 'Garganacl\n- Protect' } },
  ]) {
    const v = checkReuse(capture(p), provenanceFor(ORACLE, job));
    assert.equal(v.reuse, false);
    assert.match(v.reason, /format\/team inputs changed/);
  }
});

test('inputs digest applies the driver defaults', () => {
  const bare = { p1: JOB.p1, p2: JOB.p2 };
  assert.equal(inputsDigest(bare), inputsDigest({ ...bare, format: 'gen9customgame', seed: [1, 2, 3, 4], max_turns: 20 }));
});

test('truncated JSON: stale, not a crash', () => {
  const p = provenanceFor(ORACLE, JOB);
  const text = capture(p);
  const v = checkReuse(text.slice(0, text.length - 7), p);
  assert.equal(v.reuse, false);
  assert.match(v.reason, /truncated JSON/);
});

test('missing provenance (legacy capture): stale', () => {
  const v = checkReuse(capture(null), provenanceFor(ORACLE, JOB));
  assert.equal(v.reuse, false);
  assert.match(v.reason, /legacy capture/);
});

test('unknown or dirty current PS revision never reuses', () => {
  const p = provenanceFor(ORACLE, JOB);
  for (const rev of [null, `${ORACLE.ps_revision}-dirty`]) {
    const now = { ...p, ps_revision: rev };
    assert.equal(checkReuse(capture(now), now).reuse, false);
  }
});

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'conf-prov-'));
const git = (dir, ...a) => execFileSync('git', ['-C', dir, '-c', 'user.name=t', '-c', 'user.email=t@t', ...a], { encoding: 'utf8' }).trim();

// A minimal PS checkout: package.json named pokemon-showdown, git HEAD, and a
// dist/sim the driver can require at load time.
function fakePs() {
  const root = tmp();
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ name: 'pokemon-showdown', version: '0.11.10' }));
  git(root, 'init', '-q');
  git(root, 'add', 'package.json');
  git(root, 'commit', '-qm', 'init');
  const sim = path.join(root, 'dist', 'sim');
  fs.mkdirSync(sim, { recursive: true });
  fs.writeFileSync(path.join(sim, 'index.js'), 'module.exports = {};');
  fs.writeFileSync(path.join(sim, 'battle.js'), 'exports.Battle = class {};');
  fs.writeFileSync(path.join(sim, 'battle-actions.js'), 'exports.BattleActions = class {};');
  return { root, sim };
}

test('psRevision: git HEAD, -dirty on tracked edits, npm version without git', () => {
  const { root, sim } = fakePs();
  const head = git(root, 'rev-parse', 'HEAD');
  assert.equal(psRevision(sim), head);
  fs.writeFileSync(path.join(root, 'package.json'), JSON.stringify({ name: 'pokemon-showdown', version: '0.11.11' }));
  assert.equal(psRevision(sim), `${head}-dirty`);
  fs.rmSync(path.join(root, '.git'), { recursive: true });
  assert.equal(psRevision(sim), 'pokemon-showdown@0.11.11');
  assert.equal(psRevision(tmp()), null);
});

test('preserve moves the old capture into stale/ without overwriting', () => {
  const dir = tmp();
  const out = path.join(dir, 'out_abc.json');
  fs.writeFileSync(out, 'first');
  const a = preserve(out);
  fs.writeFileSync(out, 'second');
  const b = preserve(out);
  assert.notEqual(a, b);
  assert.equal(fs.readFileSync(a, 'utf8'), 'first');
  assert.equal(fs.readFileSync(b, 'utf8'), 'second');
  assert.equal(fs.existsSync(out), false);
  assert.deepEqual(fs.readdirSync(dir), ['stale']);
});

test('run-batch: reuses matching captures, reports stale ones and leaves them untouched', () => {
  const { sim } = fakePs();
  const work = tmp();
  const jobs = path.join(work, 'jobs');
  fs.mkdirSync(jobs);
  const driverPath = path.join(__dirname, '..', 'ps-golden-driver', 'conformance-driver.js');
  const { oracleIdentity } = require('./provenance');
  const oracle = oracleIdentity({ psDist: sim, driverPath });
  const ids = ['fresh', 'oldps', 'olddriver', 'legacy', 'trunc'];
  for (const id of ids) fs.writeFileSync(path.join(jobs, `job_${id}.json`), JSON.stringify({ ...JOB, id }));
  const p = provenanceFor(oracle, JOB);
  const outs = {
    fresh: capture(p),
    oldps: capture({ ...p, ps_revision: 'f'.repeat(40) }),
    olddriver: capture({ ...p, driver_sha256: '0'.repeat(64) }),
    legacy: capture(null),
    trunc: capture(p).slice(0, 20),
  };
  for (const id of ids) fs.writeFileSync(path.join(work, `out_${id}.json`), outs[id]);

  const r = spawnSync(process.execPath, [path.join(__dirname, 'run-batch.js')], {
    env: { ...process.env, PS_DIST: sim, CONF_JOBS_DIR: jobs, CONF_OUT_DIR: work, CONF_FORCE: '', CONF_RECAPTURE: '' },
    encoding: 'utf8',
  });
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /0 ok, 0 fail, 1 reused, 4 stale/);
  assert.match(r.stderr, /out_oldps\.json: PS revision changed/);
  assert.match(r.stderr, /out_olddriver\.json: driver hash changed/);
  assert.match(r.stderr, /out_legacy\.json: no _meta\.provenance/);
  assert.match(r.stderr, /out_trunc\.json: unreadable or truncated JSON/);
  assert.doesNotMatch(r.stderr, /out_fresh\.json:/);
  for (const id of ids) assert.equal(fs.readFileSync(path.join(work, `out_${id}.json`), 'utf8'), outs[id]);
  assert.equal(fs.existsSync(path.join(work, 'stale')), false);
});
