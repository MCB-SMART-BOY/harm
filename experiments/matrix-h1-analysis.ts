// Usage: bun experiments/matrix-h1-analysis.ts RUN_DIR
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [run] = Bun.argv.slice(2);
assert(run, "expected A4 run directory");
const arms = ["native", "static-low", "static-high", "reactive", "oracle"];
const sessionNames = ["A", "B"];
const interval = 50_000_000;
const quota = 21;
const lines = (text: string) => text.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
const read = (path: string) => lines(readFileSync(path, "utf8"));
type Trial = {
  arm: string;
  order_index: number;
  trace_file: string | { Unix: number[] };
  void_reasons: string[];
  joint_slo_pass: boolean | null;
  phase_p99_ns: { spike: number } | null;
  background_completion: { missed_deadline: boolean; elapsed_ns: number | null } | null;
  expiry_metrics: Record<string, number> | null;
  after_drain: Environment | null;
  constant_sequence_sensitivity_eligible?: boolean;
};
type Block = { seed: number; reserve: boolean; complete: boolean; trials: Trial[] };
type Environment = {
  major_faults: number | null;
  meminfo: string[];
  memory_psi: string | null;
  swap: string[];
  resident_llm: { pid: number; rss_kib: number | null; cpu_ticks: number | null }[];
  temperatures_c: Record<string, number>;
  gpu_temperature_c_and_clock_mhz: string | null;
};

function result(session: string, startSeed: number) {
  const path = join(run, "sessions", session, "session.jsonl");
  const rows = existsSync(path) ? read(path) : [];
  const start = rows.find(row => row.kind === "matrix_session_start") ?? null;
  const end = rows.find(row => row.kind === "matrix_session_end") ?? null;
  const blocks: Block[] = rows.filter(row => row.kind === "matrix_seed_block" && row.workload === "w1");
  const complete = blocks.filter(block => block.complete);
  for (const [index, block] of blocks.entries()) {
    assert.equal(block.seed, startSeed + index, session);
    assert.equal(block.reserve, index >= quota, session);
    assert(block.trials.every(trial => arms.includes(trial.arm)));
    assert.equal(new Set(block.trials.map(trial => trial.arm)).size, block.trials.length);
    assert.equal(block.complete, block.trials.length === arms.length
      && block.trials.every(trial => trial.void_reasons.length === 0), session);
  }
  if (end) assert.equal(complete.length, end.valid_blocks[0], session);
  const wrapper = join(run, `session-a4-${session}`, "environment.jsonl");
  const environment = existsSync(wrapper) ? read(wrapper) : [];
  const residencyOK = environment.some(row => row.label === "before")
    && environment.some(row => row.label === "after")
    && environment.every(row => row.label !== "residency check failed");
  return {
    session, start, end, blocks, complete, residencyOK, environment,
    cooldownStopped: rows.find(row => row.kind === "matrix_cooldown_failed") ?? null,
  };
}

function ratio(numerator: number, denominator: number) {
  return { numerator, denominator, rate: denominator ? numerator / denominator : null };
}

function percentile(values: number[], p: number) {
  if (!values.length) return null;
  return values.toSorted((a, b) => a - b)[Math.ceil(p * (values.length - 1))];
}

function mean(values: number[]) {
  return values.length ? values.reduce((a, b) => a + b, 0) / values.length : null;
}

function median(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b);
  return values.length ? (sorted[Math.floor((values.length - 1) / 2)] + sorted[Math.floor(values.length / 2)]) / 2 : null;
}

function bootstrap(strata: number[][]) {
  const n = strata.flat().length;
  if (n === 0) return null;
  let state = 20_261_124n;
  const wrap = (x: bigint) => BigInt.asUintN(64, x);
  function indexOf(size: number) {
    state = wrap(state + 0x9E3779B97F4A7C15n);
    let z = wrap((state ^ (state >> 30n)) * 0xBF58476D1CE4E5B9n);
    z = wrap((z ^ (z >> 27n)) * 0x94D049BB133111EBn);
    return Number((z ^ (z >> 31n)) % BigInt(size));
  }
  const means = Array.from({ length: 10000 }, () => {
    let sum = 0;
    for (const stratum of strata)
      for (let i = 0; i < stratum.length; i++) sum += stratum[indexOf(stratum.length)];
    return sum / n;
  });
  return [percentile(means, 0.025), percentile(means, 0.975)];
}

function score(block: Block) {
  const native = block.trials.find(trial => trial.arm === "native")!;
  const oracle = block.trials.find(trial => trial.arm === "oracle")!;
  const reference = native.phase_p99_ns?.spike;
  const test = oracle.phase_p99_ns?.spike;
  const referenceMiss = native.background_completion?.missed_deadline;
  const testMiss = oracle.background_completion?.missed_deadline;
  assert(typeof reference === "number" && typeof test === "number");
  assert(typeof referenceMiss === "boolean" && typeof testMiss === "boolean");
  assert(typeof oracle.joint_slo_pass === "boolean" && typeof native.joint_slo_pass === "boolean");
  const nativeCpuNs = native.background_completion!.elapsed_ns;
  const oracleCpuNs = oracle.background_completion!.elapsed_ns;
  const diffMs = (reference - test) / 1e6;
  return {
    seed: block.seed,
    diff_ms: diffMs,
    relative_diff_percent: reference > 0 ? 100 * (reference - test) / reference : null,
    oracle_pass: oracle.joint_slo_pass,
    native_pass: native.joint_slo_pass,
    oracle_miss: testMiss,
    native_miss: referenceMiss,
    oracle_cpu_ms: oracleCpuNs === null ? null : oracleCpuNs / 1e6,
    native_cpu_ms: nativeCpuNs === null ? null : nativeCpuNs / 1e6,
  };
}

function describe(samples: number[]) {
  return {
    count: samples.length,
    median: median(samples),
    p99: percentile(samples, 0.99),
    max: samples.reduce<number | null>((max, value) => Math.max(max ?? value, value), null),
  };
}

function controllerTiming(session: string, trials: Trial[], environment: Environment[]) {
  const ticks: number[] = [];
  const firstTicks: number[] = [];
  const applies: number[] = [];
  const activeCompute: number[] = [];
  const decisionToApply: number[] = [];
  const overheadPercent: number[] = [];
  let initialWindowMisses = 0;
  let measured = 0;
  let staleApplies = 0;
  const expiry = {
    expired_proposal_count: 0, weight_change_proposal_count: 0,
    forecasts_with_expired_proposal_count: 0, forecast_count: 0,
  };
  for (const trial of trials) {
    const name = typeof trial.trace_file === "string"
      ? trial.trace_file : Buffer.from(trial.trace_file.Unix).toString("utf8");
    const path = join(run, "sessions", session, name);
    if (!existsSync(path)) continue;
    const head = Bun.spawnSync(["head", "-n", "256", path]);
    assert.equal(head.exitCode, 0, path);
    const rows = lines(head.stdout.toString());
    const beforeWarmup = rows.find(row => row.kind === "matrix_trial_meta")?.before_warmup;
    if (beforeWarmup) environment.push(beforeWarmup);
    if (trial.after_drain) environment.push(trial.after_drain);
    if (!trial.expiry_metrics) continue;
    const tickRows = rows.filter(row => row.kind === "control_tick");
    assert.equal(tickRows.length, 240, path);
    const audit = read(path.replace(/\.jsonl$/, ".audit.jsonl"));
    for (const key of Object.keys(expiry) as Array<keyof typeof expiry>) expiry[key] += trial.expiry_metrics[key];
    const first = tickRows.find(tick => tick.applied !== null);
    firstTicks.push(tickRows[0].tick_lateness_ns);
    if (trial.arm !== "native") {
      measured++;
      if (!first || first.applied.elapsed_ns >= interval) initialWindowMisses++;
    }
    for (const tick of tickRows) {
      assert.equal(tick.tick_lateness_ns, tick.decision_start_ns - tick.scheduled_ns, path);
      ticks.push(tick.tick_lateness_ns);
      if (tick.applied?.elapsed_ns >= tick.scheduled_ns + interval) staleApplies++;
    }
    activeCompute.push(tickRows.reduce((sum, tick) => sum + tick.decision_end_ns - tick.decision_start_ns, 0));
    let trialOverheadNs = 0;
    let verified = 0;
    for (const row of audit.filter(row => row.event === "apply")) {
      const { decision_started_seconds: decision, apply_started_seconds: from, readback_completed_seconds: to } = row.action_timing;
      applies.push(Math.round((to - from) * 1e9));
      decisionToApply.push(Math.round((from - decision) * 1e9));
      trialOverheadNs += (to - decision) * 1e9;
      verified++;
    }
    if (verified) overheadPercent.push(100 * trialOverheadNs / (tickRows.length * interval));
  }
  return {
    attempts: trials.length,
    voids: trials.filter(trial => trial.void_reasons.length > 0).length,
    expired_proposals: ratio(expiry.expired_proposal_count, expiry.weight_change_proposal_count),
    forecast_expiries: ratio(expiry.forecasts_with_expired_proposal_count, expiry.forecast_count),
    initial_window_misses: ratio(initialWindowMisses, measured),
    stale_applies: ticks.length ? staleApplies : null,
    late_ticks: ratio(ticks.filter(ns => ns >= interval).length, ticks.length),
    first_tick_lateness_ns: describe(firstTicks),
    tick_lateness_ns: describe(ticks),
    apply_to_readback_ns: describe(applies),
    active_tick_compute_ns_per_trial: describe(activeCompute),
    apply_overhead_percent_of_trial: describe(overheadPercent),
    decision_to_apply_sum_ns: decisionToApply.length ? decisionToApply.reduce((sum, ns) => sum + ns, 0) : null,
    apply_to_readback_sum_ns: applies.length ? applies.reduce((sum, ns) => sum + ns, 0) : null,
  };
}

function summarizeEnvironment(samples: Environment[]) {
  const range = (values: unknown[]) => {
    const observed = values.filter((value): value is number => typeof value === "number" && Number.isFinite(value));
    return observed.length ? { min: Math.min(...observed), max: Math.max(...observed) } : null;
  };
  const counter = (rows: string[], pattern: RegExp) => {
    const match = rows.map(line => line.match(pattern)).find(result => result !== null);
    return match ? Number(match[1]) : null;
  };
  const resident = samples.flatMap(sample => sample.resident_llm ?? []);
  const gpu = samples.map(sample => sample.gpu_temperature_c_and_clock_mhz?.match(/^(\d+),\s*(\d+)/));
  const sensors = new Set(samples.flatMap(sample => Object.keys(sample.temperatures_c ?? {})));
  return {
    snapshots: samples.length,
    resident_pid: [...new Set(resident.map(row => row.pid))],
    resident_rss_kib: range(resident.map(row => row.rss_kib)),
    resident_cpu_ticks: range(resident.map(row => row.cpu_ticks)),
    available_kib: range(samples.map(row => counter(row.meminfo ?? [], /^MemAvailable:\s*(\d+)/))),
    free_kib: range(samples.map(row => counter(row.meminfo ?? [], /^MemFree:\s*(\d+)/))),
    major_faults: range(samples.map(row => row.major_faults)),
    swap_in_pages: range(samples.map(row => counter(row.swap ?? [], /^pswpin\s+(\d+)/))),
    swap_out_pages: range(samples.map(row => counter(row.swap ?? [], /^pswpout\s+(\d+)/))),
    memory_psi_full_total_us: range(samples.map(row => counter(row.memory_psi?.split("\n") ?? [], /^full.* total=(\d+)/))),
    temperatures_c: Object.fromEntries([...sensors].map(sensor =>
      [sensor, range(samples.map(row => row.temperatures_c?.[sensor]))])),
    gpu_temperature_c: range(gpu.map(match => match ? Number(match[1]) : null)),
    gpu_clock_mhz: range(gpu.map(match => match ? Number(match[2]) : null)),
  };
}

const sessions = sessionNames.map((name, i) => result(name, 2001 + 1000 * i));
const [a, b] = sessions;
const sameState = a.start && b.start && a.start.llm_state === b.start.llm_state
  && a.start.boot_id === b.start.boot_id && a.start.binary_sha256 === b.start.binary_sha256;
const gap = a.end && b.start && sameState
  ? (b.start.start_monotonic_ns - a.end.end_monotonic_ns) / 1e9
  : null;
const cohort = sessions.map(session => session.complete.map(score));
const scores = cohort.flat();
const strata = cohort.map(stratum => stratum.map(row => row.diff_ms));
const differences = strata.flat();
const meanMs = mean(differences);
const ci = bootstrap(strata);
const sd = differences.length >= 2 && meanMs !== null
  ? Math.sqrt(differences.reduce((total, x) => total + (x - meanMs) ** 2, 0) / (differences.length - 1))
  : null;
const oraclePass = ratio(scores.filter(score => score.oracle_pass).length, scores.length);
const nativePass = ratio(scores.filter(score => score.native_pass).length, scores.length);
const oracleMiss = ratio(scores.filter(score => score.oracle_miss).length, scores.length);
const nativeMiss = ratio(scores.filter(score => score.native_miss).length, scores.length);
const cpuMissDiff = cohort.map(rows => rows.map(row => 100 * (Number(row.native_miss) - Number(row.oracle_miss))));
const jointPassDiff = cohort.map(rows => rows.map(row => 100 * (Number(row.oracle_pass) - Number(row.native_pass))));
const observedCpuDiff = cohort.map(rows => rows.flatMap(row =>
  row.native_cpu_ms === null || row.oracle_cpu_ms === null ? [] : [row.native_cpu_ms - row.oracle_cpu_ms]));
const conditions = {
  quota: sessions.every(session => session.complete.length === quota && session.end !== null),
  session_gap: gap !== null && gap >= 900,
  residency: sessions.every(session => session.residencyOK),
  ci_above_zero: (ci?.[0] ?? 0) > 0,
  mean_at_least_1_ms: meanMs !== null && meanMs >= 1,
  favorable_both_sessions: strata.every(row => (mean(row) ?? 0) > 0),
  oracle_slo_at_least_90_percent: (oraclePass.rate ?? 0) >= 0.9,
  oracle_cpu_miss_rate_no_higher_than_native: scores.length > 0 && oracleMiss.numerator <= nativeMiss.numerator,
};
console.log(JSON.stringify({
  h1_pass: Object.values(conditions).every(Boolean),
  conditions,
  gap_seconds: gap,
  paired_spike_p99_ms: {
    n: differences.length, mean: meanMs, ci_95: ci,
    dz: sd !== null && sd > 0 && meanMs !== null ? meanMs / sd : null,
    median_relative_improvement_percent: median(scores.flatMap(score =>
      score.relative_diff_percent === null ? [] : [score.relative_diff_percent])),
  },
  oracle_pass: oraclePass, native_pass: nativePass, oracle_cpu_miss: oracleMiss, native_cpu_miss: nativeMiss,
  joint_pass_gain_percentage_points: { mean: mean(jointPassDiff.flat()), ci_95: bootstrap(jointPassDiff) },
  cpu_miss_gain_percentage_points: { mean: mean(cpuMissDiff.flat()), ci_95: bootstrap(cpuMissDiff) },
  cpu_completion_conditional_ms: {
    observed_pairs: observedCpuDiff.flat().length,
    native_censored: scores.filter(row => row.native_cpu_ms === null).length,
    oracle_censored: scores.filter(row => row.oracle_cpu_ms === null).length,
    paired_mean_improvement: mean(observedCpuDiff.flat()), ci_95: bootstrap(observedCpuDiff),
  },
  sessions: sessions.map(({ session, start, end, blocks, complete, residencyOK, environment, cooldownStopped }, i) => {
    const trialEnvironment: Environment[] = [];
    const controller = Object.fromEntries(arms.map(arm => [arm, controllerTiming(session,
      blocks.flatMap(block => block.trials.filter(trial => trial.arm === arm)), trialEnvironment)]));
    return {
      session, start, end, cooldown_stopped: cooldownStopped, residency_ok: residencyOK,
      complete_blocks: complete.length,
      attempted_blocks: blocks.length,
      mean_spike_p99_improvement_ms: mean(strata[i]),
      oracle_pass: ratio(cohort[i].filter(score => score.oracle_pass).length, complete.length),
      native_pass: ratio(cohort[i].filter(score => score.native_pass).length, complete.length),
      oracle_cpu_miss: ratio(cohort[i].filter(score => score.oracle_miss).length, complete.length),
      native_cpu_miss: ratio(cohort[i].filter(score => score.native_miss).length, complete.length),
      pairs: cohort[i],
      realized_order: blocks.map(({ seed, reserve, complete, trials }) =>
        ({ seed, reserve, complete, arms: trials.map(trial => trial.arm) })),
      void_ledger: blocks.flatMap(block => block.trials.filter(trial => trial.void_reasons.length > 0)
        .map(trial => ({ seed: block.seed, arm: trial.arm, reasons: trial.void_reasons }))),
      constant_sequence_sensitivity_seeds: blocks.filter(block => block.trials.some(trial =>
        trial.constant_sequence_sensitivity_eligible)).map(block => block.seed),
      controller,
      boundary_environment: environment,
      trial_environment: summarizeEnvironment(trialEnvironment),
    };
  }),
}, null, 2));
