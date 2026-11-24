// Usage: bun experiments/matrix-a3-audit.ts PILOT_DIR [STRESS_DIR [--a4]]
// Read the parent verdict (which includes scope cleanup), and independently
// reclassify the A2 pilot's controller evidence using its raw ticks and audit.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";

const [pilotDir, stressDir, amendment] = Bun.argv.slice(2);
assert(pilotDir, "expected the retained matrix-pilot-2/pilot directory");
assert(amendment === undefined || amendment === "--a4");
const arms = ["native", "static-low", "static-high", "reactive", "oracle"];
const classes = ["w1", "w2", "w3", "w4"];
const interval = 50_000_000;

function lines(input: string) {
  return input.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
}

function command(...args: string[]) {
  const result = Bun.spawnSync(args);
  assert.equal(result.exitCode, 0, `${args.join(" ")}: ${result.stderr}`);
  return result.stdout.toString();
}

function attempts(dir: string) {
  return lines(readFileSync(join(dir, "attempts.jsonl"), "utf8"))
    .filter(row => row.kind === "trial");
}

type Trial = {
  workload: string;
  seed: number;
  arm: string;
  session: string;
  void_reasons: string[];
  previous_void_reasons: string[];
  threshold_passes: Array<boolean | null>;
  expiry_metrics: ExpiryMetrics;
  initial_verified_ns: number | null;
  initial_window_miss: boolean | null;
  stale_applied: { scheduled_ns: number; verified_ns: number }[];
  tick_lateness_ns: number[];
  apply_to_readback_ns: number[];
};

type ExpiryMetrics = {
  expired_proposal_count: number;
  weight_change_proposal_count: number;
  forecasts_with_expired_proposal_count: number;
  forecast_count: number;
};

function auditTrial(dir: string, attempt: { class: string; arm: string; seed: number; session: string }, historical: boolean): Trial {
  const { class: workload, arm, seed, session } = attempt;
  const path = join(dir, `feasibility-${session}`, `matrix-${workload}-${arm}-${seed}.jsonl`);
  const [verdict] = lines(command("tail", "-n", "1", path));
  assert.equal(verdict.kind, "matrix_trial_verdict", path);
  assert.deepEqual([verdict.workload, verdict.arm, verdict.seed, verdict.session], [workload, arm, seed, session], path);
  const expiry_metrics: ExpiryMetrics = {
    expired_proposal_count: 0,
    weight_change_proposal_count: 0,
    forecasts_with_expired_proposal_count: 0,
    forecast_count: 0,
  };
  const stale_applied: Trial["stale_applied"] = [];
  const tick_lateness_ns: number[] = [];
  const apply_to_readback_ns: number[] = [];
  const previous_void_reasons: string[] = verdict.void_reasons;
  const void_reasons = [...previous_void_reasons];
  const ticks = lines(command("head", "-n", "256", path)).filter(row => row.kind === "control_tick");
  if (!ticks.length && amendment === "--a4" && !historical && void_reasons.length > 0)
    return { workload, seed, arm, session, void_reasons, previous_void_reasons,
      threshold_passes: verdict.threshold_passes, expiry_metrics, initial_verified_ns: null,
      initial_window_miss: null, stale_applied, tick_lateness_ns, apply_to_readback_ns };
  assert.equal(ticks.length, 240, path);
  const audit = lines(readFileSync(path.replace(/\.jsonl$/, ".audit.jsonl"), "utf8"));
  const writes = audit.filter(row => row.event === "apply");
  const initialWeight = arm === "native" ? null : arm === "static-low" ? 20 : 1000;
  let initial_verified_ns = null;
  let weight = 100;
  const sequence = [];
  let rejectedRequiredWrite = false;
  for (const [index, tick] of ticks.entries()) {
    assert.equal(tick.scheduled_ns, index * interval, path);
    const late = tick.decision_start_ns - tick.scheduled_ns;
    assert(late >= 0, path);
    if (!historical && amendment === "--a4") assert.equal(tick.tick_lateness_ns, late, path);
    tick_lateness_ns.push(late);
    const proposed = tick.proposed_weight !== null && tick.proposed_weight !== weight;
    const expired = tick.outcome === (historical ? "expired" : "expired_proposal");
    const forecast = tick.forecast_ns !== null;
    expiry_metrics.weight_change_proposal_count += Number(proposed);
    expiry_metrics.expired_proposal_count += Number(expired);
    expiry_metrics.forecasts_with_expired_proposal_count += Number(expired && forecast);
    expiry_metrics.forecast_count += Number(forecast);
    if (expired) assert(proposed && tick.applied === null, path);
    if (tick.outcome === "apply_mismatch" || (tick.outcome === "apply_rejected" && tick.error !== "RateLimited"))
      rejectedRequiredWrite = true;
    if (tick.applied !== null) {
      const { before, readback, elapsed_ns } = tick.applied;
      const row = writes[sequence.length];
      assert(proposed && before === weight && readback === tick.proposed_weight, path);
      assert(row?.result === "applied" && row.before === before && row.readback === readback
        && row.after === readback && row.requested === readback && row.action_timing, path);
      const timing = row.action_timing;
      assert(timing.decision_started_seconds <= timing.apply_started_seconds
        && timing.apply_started_seconds <= timing.readback_completed_seconds
        && timing.readback_completed_seconds <= row.monotonic_seconds, path);
      apply_to_readback_ns.push(Math.round((timing.readback_completed_seconds - timing.apply_started_seconds) * 1e9));
      if (elapsed_ns >= tick.scheduled_ns + interval)
        stale_applied.push({ scheduled_ns: tick.scheduled_ns, verified_ns: elapsed_ns });
      initial_verified_ns ??= elapsed_ns;
      sequence.push(readback);
      weight = readback;
    }
    assert.equal(tick.effective_weight, weight, path);
  }
  assert.equal(writes.length, sequence.length, path);
  assert.deepEqual(verdict.effective_weight_sequence, sequence, path);
  assert.equal(verdict.control_actions, writes.length, path);
  const initialOK = initialWeight === null ? !sequence.length
    : sequence[0] === initialWeight && initial_verified_ns !== null && initial_verified_ns < interval;
  const restored = sequence.length === 0 || audit.some(
    row => row.event === "restore" && row.result === "restored" && row.readback === 100,
  );
  if (historical) {
    const pos = void_reasons.indexOf("missing_verified_apply");
    if (pos >= 0 && initialOK && restored && !rejectedRequiredWrite
      && !audit.some(row => row.event === "rollback") && expiry_metrics.expired_proposal_count > 0)
      void_reasons.splice(pos, 1);
    if (stale_applied.length) void_reasons.push("stale_applied_decision");
  } else {
    assert.deepEqual(verdict.expiry_metrics, expiry_metrics, path);
    assert.equal(void_reasons.includes("stale_applied_decision"), stale_applied.length > 0, path);
    if (amendment === "--a4") {
      const [meta] = lines(command("head", "-n", "1", path));
      assert.deepEqual(meta.controller_observer_cores, [0, 1], path);
      assert.equal(meta.cores, "2-3", path);
      assert.deepEqual(verdict.controller_tick_timing, {
        tick_count: ticks.length,
        first_lateness_ns: tick_lateness_ns[0],
        ...Object.fromEntries(Object.entries(distribution(tick_lateness_ns))
          .filter(([key]) => key !== "count").map(([key, value]) => [key.replace("_ns", "_lateness_ns"), value])),
        ticks_at_or_after_expiry: tick_lateness_ns.filter(ns => ns >= interval).length,
      }, path);
      assert.equal(verdict.controller_overhead?.p99_apply_to_readback_ns ?? null,
        distribution(apply_to_readback_ns).p99_ns, path);
    }
  }
  return { workload, seed, arm, session, void_reasons, previous_void_reasons,
    threshold_passes: verdict.threshold_passes, expiry_metrics, initial_verified_ns,
    initial_window_miss: initialWeight === null ? null : !initialOK, stale_applied,
    tick_lateness_ns, apply_to_readback_ns };
}

function distribution(samples: number[]) {
  const sorted = samples.toSorted((a, b) => a - b);
  return {
    count: samples.length,
    p99_ns: sorted[Math.ceil(0.99 * (sorted.length - 1))] ?? null,
    max_ns: sorted.at(-1) ?? null,
  };
}

function summarize(trials: Trial[], arm: string) {
  const rows = trials.filter(row => row.arm === arm);
  const voids = rows.filter(row => row.void_reasons.length).length;
  const sum = (field: keyof ExpiryMetrics) => rows.reduce((total, row) => total + row.expiry_metrics[field], 0);
  const ratio = (numerator: number, denominator: number) => ({ numerator, denominator, rate: denominator ? numerator / denominator : null });
  const ticks = rows.flatMap(row => row.tick_lateness_ns);
  return {
    attempts: rows.length, valid: rows.length - voids, voids, void_rate: voids / rows.length,
    expiry: ratio(sum("expired_proposal_count"), sum("weight_change_proposal_count")),
    forecast_expiry: ratio(sum("forecasts_with_expired_proposal_count"), sum("forecast_count")),
    initial_window_misses: ratio(rows.filter(r => r.initial_window_miss === true).length,
      rows.filter(r => r.initial_window_miss !== null).length),
    late_ticks: ratio(ticks.filter(ns => ns >= interval).length, ticks.length),
    tick_lateness: distribution(ticks),
    apply_to_readback: distribution(rows.flatMap(row => row.apply_to_readback_ns)),
    stale_applied: rows.reduce((total, row) => total + row.stale_applied.length, 0),
  };
}

const pilot = attempts(pilotDir).map(attempt => auditTrial(pilotDir, attempt, true));
const blockCount: Record<string, number> = {};
const headroom: Record<string, { passes: number[]; misses: number[]; eligible: boolean }> = {};
for (const workload of classes) {
  blockCount[workload] = 0;
  for (let seed = 1001; seed <= 1006; seed++) {
    const rows = pilot.filter(row => row.workload === workload && row.seed === seed);
    assert.deepEqual(rows.map(row => row.arm).sort(), [...arms].sort());
    blockCount[workload] += Number(rows.every(row => !row.void_reasons.length));
  }
  const valid = pilot.filter(row => row.workload === workload && !row.void_reasons.length);
  const passes = [0, 1, 2].map(index => valid.filter(row => row.threshold_passes[index] === true).length);
  const misses = [0, 1, 2].map(index => valid.filter(row => row.threshold_passes[index] === false).length);
  headroom[workload] = { passes, misses, eligible: passes.every(Boolean) && misses.every(Boolean) };
}
assert.equal(pilot.length, 120);
const pilotRates = Object.fromEntries(arms.map(arm => [arm, summarize(pilot, arm)]));
const pilotW1Rates = Object.fromEntries(arms.map(arm => [arm, summarize(pilot.filter(row => row.workload === "w1"), arm)]));
const estimatedYield = 27 * Object.values(pilotRates).reduce((yieldPerSeed, arm) => yieldPerSeed * (1 - arm.void_rate), 1);
const stress = stressDir ? attempts(stressDir).map(attempt => auditTrial(stressDir, attempt, false)) : null;
const stressRates = stress && Object.fromEntries(["reactive", "oracle"].map(arm => {
  const rows = stress.filter(row => row.arm === arm);
  assert.deepEqual(rows.map(row => row.session), Array.from({ length: 20 }, (_, i) => `stress-${arm}-${i + 1}`));
  assert(rows.every(row => row.workload === "w1" && row.seed === 2001));
  return [arm, summarize(rows, arm)];
}));
if (stress) assert.equal(stress.length, 40);
const passesValidity = (rates: typeof pilotRates) => Object.values(rates).every(arm => arm.void_rate <= 0.05);
const a4Rates = stressRates && { ...pilotW1Rates, ...stressRates };
const a4Yield = a4Rates && 27 * Object.values(a4Rates).reduce((yieldPerSeed, arm) => yieldPerSeed * (1 - arm.void_rate), 1);
const a4Late = stress?.filter(row => row.initial_window_miss);
const trialLedger = (trials: Trial[]) => trials.map(({tick_lateness_ns, apply_to_readback_ns, ...trial}) => ({
  ...trial,
  first_tick_lateness_ns: tick_lateness_ns[0] ?? null,
  tick_lateness: distribution(tick_lateness_ns),
  apply_to_readback: distribution(apply_to_readback_ns),
}));
console.log(JSON.stringify({
  pilot: { rates: pilotRates, w1_rates: pilotW1Rates, block_count: blockCount, headroom, estimated_yield: estimatedYield, trials: trialLedger(pilot) },
  stress: stress && { rates: stressRates, trials: trialLedger(stress) },
  ...amendment === "--a4" && { a4_gate: { rates: a4Rates, estimated_yield: a4Yield, initial_window_miss_trials: a4Late?.map(row => row.session) ?? null } },
  stage_0_pass: stressRates === null ? null : amendment === "--a4"
    ? headroom.w1.eligible && passesValidity(a4Rates!) && a4Yield! >= 21 && a4Late!.length === 0
    : passesValidity(pilotRates) && passesValidity(stressRates) && estimatedYield >= 21,
}, null, 2));
