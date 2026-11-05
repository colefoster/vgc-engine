'use strict';
// In-process batch driver: load PS once, run every job_<id>.json, write the
// matching out_<id>.json. Battle IDs are stable content hashes (see mkjobs.py),
// so re-running only the NEW jobs is safe. An existing out_<id>.json is reused
// only when its `_meta.provenance` matches the current PS revision, driver
// hash and job inputs (see provenance.js). Anything else is reported STALE and
// left untouched unless CONF_RECAPTURE=1, which moves it to stale/ and
// re-captures it.
//
// Env:
//   CONF_JOBS_DIR   jobs input dir   (default /tmp/conf-batch/jobs)
//   CONF_OUT_DIR    outputs dir      (default /tmp/conf-batch)
//   CONF_RECAPTURE  '1' to re-capture stale outputs (old file kept in stale/)
//   CONF_FORCE      '1' to re-run every job (old files kept in stale/)
const fs = require('fs');
const path = require('path');
const driverPath = path.join(__dirname, '..', 'ps-golden-driver', 'conformance-driver.js');
const { runJob, PS_PATH } = require(driverPath);
const { oracleIdentity, provenanceFor, checkReuse, preserve } = require('./provenance');

const jobsDir = process.env.CONF_JOBS_DIR || '/tmp/conf-batch/jobs';
const outDir = process.env.CONF_OUT_DIR || '/tmp/conf-batch';
const force = process.env.CONF_FORCE === '1';
const recapture = process.env.CONF_RECAPTURE === '1';

(async () => {
  fs.mkdirSync(outDir, { recursive: true });
  const jobs = fs
    .readdirSync(jobsDir)
    .filter((f) => f.startsWith('job_') && f.endsWith('.json'))
    .sort();
  let ok = 0, fail = 0, skipped = 0;
  const stale = [];
  const oracle = oracleIdentity({ psDist: PS_PATH, driverPath });
  process.stderr.write(`oracle: PS ${oracle.ps_revision || 'unknown'}, driver ${oracle.driver_sha256.slice(0, 12)}\n`);
  for (const jf of jobs) {
    const id = jf.replace(/^job_/, '').replace(/\.json$/, '');
    const outPath = path.join(outDir, `out_${id}.json`);
    const job = JSON.parse(fs.readFileSync(path.join(jobsDir, jf), 'utf8'));
    const provenance = provenanceFor(oracle, job);
    const exists = fs.existsSync(outPath) && fs.statSync(outPath).size > 0;
    if (exists && !force) {
      const verdict = checkReuse(fs.readFileSync(outPath, 'utf8'), provenance);
      if (verdict.reuse) {
        skipped++;
        continue;
      }
      if (!recapture) {
        stale.push(`out_${id}.json: ${verdict.reason}`);
        continue;
      }
    }
    try {
      const result = await runJob(job);
      result.id = id; // carry the stable id into the output for reference
      result._meta = Object.assign(result._meta || {}, { provenance });
      if (exists) process.stderr.write(`out_${id}: previous capture kept at ${preserve(outPath)}\n`);
      fs.writeFileSync(outPath, JSON.stringify(result));
      const meta = result._meta;
      process.stderr.write(
        `out_${id}: ${result.turns.length} turns, ${meta.totalDraws} draws` +
          `${meta.ok === false ? ' [SIDE ERR]' : ''}\n`,
      );
      ok++;
    } catch (e) {
      process.stderr.write(`out_${id}: FAIL ${String((e && e.message) || e)}\n`);
      fail++;
    }
  }
  if (stale.length) {
    process.stderr.write(`\nSTALE (${stale.length}, not reused, not overwritten):\n`);
    for (const s of stale) process.stderr.write(`  ${s}\n`);
    process.stderr.write('Re-capture them with CONF_RECAPTURE=1 (old files move to stale/).\n');
  }
  process.stderr.write(
    `\nbatch done: ${ok} ok, ${fail} fail, ${skipped} reused, ${stale.length} stale\n`,
  );
  if (stale.length) process.exitCode = 1;
})();
