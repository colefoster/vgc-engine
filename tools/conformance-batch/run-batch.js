'use strict';
// In-process batch driver: load PS once, run every job_<id>.json, write the
// matching out_<id>.json. Battle IDs are stable content hashes (see mkjobs.py),
// so re-running only the NEW jobs is safe — by default we skip a job whose
// out_<id>.json already exists (set CONF_FORCE=1 to re-run all).
//
// Env:
//   CONF_JOBS_DIR  jobs input dir   (default /tmp/conf-batch/jobs)
//   CONF_OUT_DIR   outputs dir      (default /tmp/conf-batch)
//   CONF_FORCE     '1' to re-run jobs whose out file already exists
const fs = require('fs');
const path = require('path');
const { runJob } = require(
  path.join(__dirname, '..', 'ps-golden-driver', 'conformance-driver.js'),
);

const jobsDir = process.env.CONF_JOBS_DIR || '/tmp/conf-batch/jobs';
const outDir = process.env.CONF_OUT_DIR || '/tmp/conf-batch';
const force = process.env.CONF_FORCE === '1';

(async () => {
  fs.mkdirSync(outDir, { recursive: true });
  const jobs = fs
    .readdirSync(jobsDir)
    .filter((f) => f.startsWith('job_') && f.endsWith('.json'))
    .sort();
  let ok = 0, fail = 0, skipped = 0;
  for (const jf of jobs) {
    const id = jf.replace(/^job_/, '').replace(/\.json$/, '');
    const outPath = path.join(outDir, `out_${id}.json`);
    if (!force && fs.existsSync(outPath) && fs.statSync(outPath).size > 0) {
      skipped++;
      continue;
    }
    const job = JSON.parse(fs.readFileSync(path.join(jobsDir, jf), 'utf8'));
    try {
      const result = await runJob(job);
      result.id = id; // carry the stable id into the output for reference
      fs.writeFileSync(outPath, JSON.stringify(result));
      const meta = result._meta || {};
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
  process.stderr.write(
    `\nbatch done: ${ok} ok, ${fail} fail, ${skipped} skipped (already present)\n`,
  );
})();
