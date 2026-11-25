// Usage: bun experiments/matrix-a6-analysis.ts experiments/data/a6-2026-11-25
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [root] = Bun.argv.slice(2);
assert(root, "expected the A6 run directory");

const arms = ["native", "static-low", "reactive", "oracle", "declared-250", "declared-500"];
const phases = ["burst", "spike"];

type Phase = {
  phase: string;
  onset_target_ns: number;
  observed_onset_ns: number | null;
  onset_source: string;
  send_target_ns: number | null;
  accepted_intent: {
    intent: { onset_target_ns: number; sent_ns: number };
    received_ns: number;
    disposition: string;
    source: string;
  } | null;
  verified_protection_ns: number | null;
  expired_protection_proposal: boolean | null;
  warning_lead_ns: number | null;
  effective_lead_ns: number | null;
};

type Trial = {
  arm: string;
  seed: number;
  void_reasons: string[];
  threshold_passes?: [boolean, boolean, boolean];
  joint_slo_pass: boolean | null;
  phase_p99_ns: { burst: number | null; spike: number | null } | null;
  background_completion: {
    missed_deadline: boolean;
    elapsed_ns: number | null;
    censored_at_ns: number | null;
  } | null;
  expiry_metrics: { expired_proposal_count: number } | null;
  effective_weight_sequence?: number[];
  a6_leadtime?: {
    gate_ns: number;
    phases: Phase[];
    false_alarm_count: number | null;
    verified_actions: {
      source: string;
      verified_ns: number;
      weight: number;
      declared_onset_target_ns: number | null;
    }[];
  };
};

type Block = { seed: number; complete: boolean; trials: Trial[] };
type SessionRow = { kind: string; workload?: string; valid_blocks?: number[] } & Block;

function readSession(name: string) {
  const path = join(root, name, "session.jsonl");
  if (!existsSync(path)) return null;
  const rows: SessionRow[] = readFileSync(path, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  const blocks = rows.filter((row) => row.kind === "matrix_seed_block");
  for (const block of blocks) {
    assert.equal(block.workload, "w1");
    assert(block.trials.every((row) => arms.includes(row.arm) && row.seed === block.seed));
    assert.equal(new Set(block.trials.map((row) => row.arm)).size, block.trials.length);
    assert.equal(block.complete, block.trials.length === arms.length && block.trials.every(valid));
    for (const trial of block.trials.filter(valid)) validateTimings(trial);
  }
  const complete = blocks.filter((block) => block.complete);
  const end = rows.find((row) => row.kind === "matrix_session_end");
  if (end) assert.equal(end.valid_blocks?.[0], complete.length);
  return { blocks, complete, ended: end !== undefined, attempted: blocks.flatMap((block) => block.trials) };
}

function valid(row: Trial) {
  return row.void_reasons.length === 0;
}

function validateTimings(row: Trial) {
  const evidence = row.a6_leadtime;
  assert(evidence, `missing A6 timing evidence for ${row.arm}, seed ${row.seed}`);
  assert.deepEqual(evidence.phases.map((phase) => phase.phase), phases);
  for (const phase of evidence.phases) {
    const onset = phase.observed_onset_ns;
    assert(onset != null && onset >= phase.onset_target_ns);
    assert.equal(phase.onset_source, "observed");
    if (!row.arm.startsWith("declared-")) continue;

    const delivery = phase.accepted_intent;
    assert(delivery && phase.send_target_ns != null);
    const target = phase.onset_target_ns;
    const expectedLeadNs = Number(row.arm.slice("declared-".length)) * 1e6;
    assert.equal(target - phase.send_target_ns, expectedLeadNs);
    assert.equal(delivery.disposition, "accepted");
    assert.equal(delivery.source, "observed");
    assert.equal(delivery.intent.onset_target_ns, target);
    assert(phase.send_target_ns <= delivery.intent.sent_ns);
    assert(delivery.intent.sent_ns <= delivery.received_ns && delivery.received_ns < target);
    assert.equal(phase.warning_lead_ns, onset - delivery.received_ns);
    const applied = phase.verified_protection_ns;
    if (applied == null) {
      assert(phase.expired_protection_proposal);
      continue;
    }
    assert(delivery.received_ns <= applied && applied < target);
    assert.equal(phase.effective_lead_ns, onset - applied);
    assert(evidence.verified_actions.some((action) => action.verified_ns === applied
      && action.source === "predicted" && action.declared_onset_target_ns === target));
  }
}

function percentile(sorted: number[], p: number) {
  return sorted.length ? sorted[Math.ceil(p * (sorted.length - 1))] : null;
}

function percentiles(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b);
  return {
    count: sorted.length,
    min: sorted[0] ?? null,
    p50: percentile(sorted, 0.50),
    p95: percentile(sorted, 0.95),
    p99: percentile(sorted, 0.99),
    max: sorted.at(-1) ?? null,
  };
}

function summarize(attempted: Trial[], included: Trial[]) {
  return Object.fromEntries(arms.map((arm) => {
    const attempts = attempted.filter((row) => row.arm === arm);
    const rows = included.filter((row) => row.arm === arm);
    const evidence = attempts.flatMap((row) => row.a6_leadtime?.phases ?? []);
    const alarms = attempts.flatMap((row) => row.a6_leadtime?.false_alarm_count ?? []);
    const expiries = attempts.flatMap((row) => row.expiry_metrics?.expired_proposal_count ?? []);
    const voidReasons: Record<string, number> = {};
    const actionSequences: Record<string, number> = {};
    for (const row of attempts) {
      for (const reason of row.void_reasons) voidReasons[reason] = (voidReasons[reason] ?? 0) + 1;
      if (row.effective_weight_sequence) {
        const key = row.effective_weight_sequence.join("→") || "no writes";
        actionSequences[key] = (actionSequences[key] ?? 0) + 1;
      }
    }
    const foreground = (row: Trial) => {
      assert(row.threshold_passes && row.background_completion && row.phase_p99_ns);
      return row.threshold_passes;
    };
    return [arm, {
      attempted: attempts.length,
      valid: attempts.filter(valid).length,
      included: rows.length,
      voidReasons,
      burstPass: rows.filter((row) => foreground(row)[0]).length,
      spikePass: rows.filter((row) => foreground(row)[1]).length,
      latencyPass: rows.filter((row) => foreground(row)[0] && foreground(row)[1]).length,
      deadlinePass: rows.filter((row) => foreground(row)[2]).length,
      jointPass: rows.filter((row) => row.joint_slo_pass).length,
      burstP99Ns: percentiles(rows.flatMap((row) => row.phase_p99_ns?.burst ?? [])),
      spikeP99Ns: percentiles(rows.flatMap((row) => row.phase_p99_ns?.spike ?? [])),
      completionNs: percentiles(rows.flatMap((row) => row.background_completion?.elapsed_ns ?? [])),
      censoredAtNs: percentiles(rows.flatMap((row) => row.background_completion?.censored_at_ns ?? [])),
      actionSequences,
      falseAlarms: alarms.length ? alarms.reduce((a, b) => a + b, 0) : null,
      expiries: expiries.length ? expiries.reduce((a, b) => a + b, 0) : null,
      deliveryNs: percentiles(evidence.flatMap(({ accepted_intent: accepted }) =>
        accepted ? [accepted.received_ns - accepted.intent.sent_ns] : [])),
      sendLatenessNs: percentiles(evidence.flatMap((phase) =>
        phase.accepted_intent && phase.send_target_ns != null
          ? [phase.accepted_intent.intent.sent_ns - phase.send_target_ns] : [])),
      onsetLatenessNs: percentiles(evidence.flatMap((phase) =>
        phase.observed_onset_ns == null ? [] : [phase.observed_onset_ns - phase.onset_target_ns])),
      warningLeadNs: percentiles(evidence.flatMap((phase) => phase.warning_lead_ns ?? [])),
      effectiveLeadNs: percentiles(evidence.flatMap((phase) => phase.effective_lead_ns ?? [])),
      leadByPhaseNs: Object.fromEntries(phases.map((name) =>
        [name, percentiles(evidence.filter((phase) => phase.phase === name).flatMap((phase) => phase.effective_lead_ns ?? []))])),
    }];
  }));
}

function pairedLeads(blocks: Block[]) {
  const rows = blocks.flatMap((block) => phases.flatMap((phase) => {
    const lead = (arm: string) => block.trials.find((row) => row.arm === arm)
      ?.a6_leadtime?.phases.find((row) => row.phase === phase)?.effective_lead_ns;
    const shortLead = lead("declared-250");
    const longLead = lead("declared-500");
    return shortLead != null && longLead != null ? [{
      seed: block.seed, phase, shortLead, longLead, difference: longLead - shortLead,
    }] : [];
  }));
  return {
    longerLead: rows.filter((row) => row.difference > 0).length,
    differenceNs: percentiles(rows.map((row) => row.difference)),
    rows,
  };
}

type Differences = { p99Ms: number; deadlineMissPp: number };

function difference(block: Block): Differences {
  const short = block.trials.find((row) => row.arm === "declared-250");
  const long = block.trials.find((row) => row.arm === "declared-500");
  assert(short?.phase_p99_ns && long?.phase_p99_ns && short.background_completion && long.background_completion);
  const worst = (row: NonNullable<Trial["phase_p99_ns"]>) => {
    assert(row.burst !== null && row.spike !== null);
    return Math.max(row.burst, row.spike);
  };
  return {
    p99Ms: (worst(long.phase_p99_ns) - worst(short.phase_p99_ns)) / 1e6,
    deadlineMissPp: (Number(long.background_completion.missed_deadline)
      - Number(short.background_completion.missed_deadline)) * 100,
  };
}

function average(rows: Differences[]) {
  return {
    p99Ms: rows.reduce((sum, row) => sum + row.p99Ms, 0) / rows.length,
    deadlineMissPp: rows.reduce((sum, row) => sum + row.deadlineMissPp, 0) / rows.length,
  };
}

function bootstrap(strata: Differences[][]) {
  let state = 20261125n;
  const u64 = (x: bigint) => BigInt.asUintN(64, x);
  function draw(size: number) {
    state = u64(state + 0x9E3779B97F4A7C15n);
    let x = state;
    x = u64((x ^ (x >> 30n)) * 0xBF58476D1CE4E5B9n);
    x = u64((x ^ (x >> 27n)) * 0x94D049BB133111EBn);
    return Number((x ^ (x >> 31n)) % BigInt(size));
  }
  const draws = Array.from({ length: 20_000 }, () => average(strata.flatMap(
    (rows) => rows.map(() => rows[draw(rows.length)]),
  )));
  function interval(endpoint: keyof Differences) {
    const sorted = draws.map((row) => row[endpoint]).sort((a, b) => a - b);
    return { lower: percentile(sorted, 0.0125)!, upper: percentile(sorted, 0.9875)! };
  }
  const p99Ms = interval("p99Ms");
  const deadlineMissPp = interval("deadlineMissPp");
  return {
    estimate: average(strata.flat()),
    bySession: strata.map(average),
    p99Ms,
    deadlineMissPp,
    favorable: p99Ms.upper <= 0.10 && deadlineMissPp.upper <= 5
      && (p99Ms.upper < 0 || deadlineMissPp.upper < 0),
  };
}

const pilot = readSession("pilot");
assert(pilot, "missing pilot manifest");
const pilotArms = summarize(pilot.attempted, pilot.attempted.filter(valid));
const pilotGate = pilot.ended
  ? JSON.parse(readFileSync(join(root, "pilot", "a6-pilot-validity.json"), "utf8"))
  : null;
if (pilotGate) {
  // The runner retains partial cooldown-stopped blocks in the manifest, but
  // does not include them in the gate's registered counts.
  const finishedBlocks = pilot.blocks.filter((block) => block.trials.length === arms.length);
  const pilotOffsets = phases.map(() => new Set<number>());
  let allDeliveries = finishedBlocks.length === 6;
  for (const block of finishedBlocks) {
    for (const arm of ["declared-250", "declared-500"]) {
      const evidence = block.trials.find((row) => row.arm === arm)?.a6_leadtime;
      for (const [index, phase] of phases.entries()) {
        const timing = evidence?.phases.find((row) => row.phase === phase);
        if (evidence && timing?.accepted_intent?.disposition === "accepted") {
          pilotOffsets[index].add(timing.onset_target_ns - evidence.gate_ns);
        } else {
          allDeliveries = false;
        }
      }
    }
  }
  const distinctOnsets = pilotOffsets.map((offsets) => offsets.size);
  const randomizedDelivery = allDeliveries && distinctOnsets.every((count) => count === 6);
  const gateArms = summarize([], finishedBlocks.flatMap((block) => block.trials.filter(valid)));
  const headroom = Object.values(gateArms).some((arm) => arm.jointPass > 0 && arm.jointPass < arm.included);
  assert.deepEqual(pilotGate.distinct_onset_counts, distinctOnsets);
  assert.equal(pilotGate.randomized_delivery, randomizedDelivery);
  assert.equal(pilotGate.joint_headroom, headroom);
  assert.equal(pilotGate.eligible, pilot.complete.length === 6 && randomizedDelivery && headroom);
  for (const [arm, summary] of Object.entries(gateArms)) {
    assert.deepEqual(pilotGate.arms[arm], {
      valid_count: summary.included,
      void_or_missing_count: 6 - summary.included,
      joint_pass: summary.jointPass,
      joint_miss: summary.included - summary.jointPass,
    });
  }
}
const a = readSession("A");
const b = readSession("B");
const scored = a?.ended && b?.ended && a.complete.length === 21 && b.complete.length === 21
  ? {
    arms: summarize([...a.attempted, ...b.attempted], [...a.complete, ...b.complete].flatMap((row) => row.trials)),
    pairedProtectionLead: pairedLeads([...a.complete, ...b.complete]),
    primary: bootstrap([a.complete.map(difference), b.complete.map(difference)]),
  }
  : null;
console.log(JSON.stringify({
  pilot: {
    ended: pilot.ended,
    completeBlocks: pilot.complete.length,
    gate: pilotGate,
    arms: pilotArms,
    pairedProtectionLead: pairedLeads(pilot.blocks),
  },
  scored,
}, null, 2));
