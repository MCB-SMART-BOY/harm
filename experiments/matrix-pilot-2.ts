// Usage: taskset -c 4 bun experiments/matrix-pilot-2.ts MODE BINARY CONFIG OUTPUT
// Modes: calibrate, stress, pilot, stress-a3, stress-a4, prepare-a4-A/B, session-a4-A/B.
import { appendFileSync, mkdirSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

const [mode, executable, configPath, output] = Bun.argv.slice(2);
const scored = /^(prepare|session)-a4-(A|B)$/.exec(mode);
if ((!scored && !["calibrate", "stress", "pilot", "stress-a3", "stress-a4"].includes(mode)) || !executable || !configPath || !output)
  throw new Error("expected MODE BINARY CONFIG OUTPUT");
const binary = resolve(executable);
const cfgPath = resolve(configPath);
const cfg = await Bun.file(cfgPath).json();
const scoredStress = mode === "stress-a3" || mode === "stress-a4";
if (cfg.matrix.stage !== (scoredStress || scored ? "full" : "pilot"))
  throw new Error("expected the registered config for this mode");
const dir = resolve(output, mode);
mkdirSync(dir);
const classes = ["w1", "w2", "w3", "w4"];

function command(...args: string[]): string {
  const result = Bun.spawnSync(args);
  if (result.exitCode !== 0)
    throw new Error(`${args.join(" ")}: ${result.stderr.toString().trim()}`);
  return result.stdout.toString().trimEnd();
}

const pid = command("pgrep", "-u", command("id", "-u"), "-x", "llama-server");
if (!/^\d+$/.test(pid)) throw new Error("expected exactly one resident llama-server");
const read = (path: string) => readFileSync(path, "utf8").trimEnd();
const modelStat = () => read(`/proc/${pid}/stat`).split(") ").at(-1)!.split(" ")[19];
type LoadedModel = {
  identifier: string; modelKey: string; sizeBytes: number; contextLength: number;
  status: string; queued: number; lastUsedTime: number;
};
function models(): LoadedModel[] {
  const loaded: LoadedModel[] = JSON.parse(command("timeout", "5s", "lms", "ps", "--json"));
  if (loaded.length !== 1 || loaded[0].status !== "idle" || loaded[0].queued !== 0)
    throw new Error(`expected one idle model without queued requests, got ${JSON.stringify(loaded)}`);
  return loaded;
}
const signature = (loaded: LoadedModel[]) => {
  const { identifier, modelKey, sizeBytes, contextLength, lastUsedTime } = loaded[0];
  return JSON.stringify([identifier, modelKey, sizeBytes, contextLength, lastUsedTime, modelStat()]);
};
const initial = signature(models());
const binarySha = command("sha256sum", binary).split(" ")[0];
if (scored) {
  const gate = JSON.parse(readFileSync(resolve(output, "stage0-a4.json"), "utf8"));
  if (gate.stage_0_pass !== true) throw new Error("A4 Stage 0 must pass before a scored run");
  const stressRoot = resolve(output, "stress-a4");
  const run = JSON.parse(readFileSync(resolve(stressRoot, "run.jsonl"), "utf8").split("\n")[0]);
  if (run.sha256 !== binarySha || JSON.stringify(run.config) !== JSON.stringify(cfg)) {
    throw new Error("scored run requires the binary and config used in Stage 0");
  }
}
function check() {
  const loaded = models();
  const current = signature(loaded);
  if (current !== initial)
    throw new Error(`resident model identity, last use, or PID changed: ${initial} → ${current}`);
  return loaded;
}
function log(file: string, value: object) {
  appendFileSync(`${dir}/${file}.jsonl`, `${JSON.stringify(value)}\n`);
}
function snapshot(label: string) {
  log("environment", {
    label, timestamp: new Date().toISOString(), pid, lms_json: check(),
    lms_ps: command("timeout", "5s", "lms", "ps"), free_g: command("free", "-g"),
    loadavg: read("/proc/loadavg"),
    psi: Object.fromEntries(["cpu", "memory", "io"].map(type => [type, read(`/proc/pressure/${type}`)])),
    ...(scoredStress || scored ? {
      temperatures_c: Object.fromEntries(temperatures()),
      kernel: read("/proc/sys/kernel/osrelease"),
      power_profile: optionalRead("/sys/firmware/acpi/platform_profile"),
      ac_online: optionalRead("/sys/class/power_supply/AC0/online"),
      cpu_governors: [0, 1, 2, 3].map(cpu =>
        optionalRead(`/sys/devices/system/cpu/cpu${cpu}/cpufreq/scaling_governor`)),
      gpu_state: optionalCommand("timeout", "5s", "nvidia-smi",
        "--query-gpu=name,driver_version,temperature.gpu,clocks.sm,pstate", "--format=csv,noheader"),
    } : {}),
  });
}

function optionalCommand(...args: string[]) {
  const result = Bun.spawnSync(args);
  return result.exitCode === 0 ? result.stdout.toString().trimEnd() : null;
}

function optionalRead(path: string) {
  try {
    return read(path);
  } catch {
    return null;
  }
}

function temperatures() {
  const values = new Map<string, number>();
  for (const name of readdirSync("/sys/class/hwmon")) {
    const path = `/sys/class/hwmon/${name}`;
    if (!["coretemp", "k10temp", "acpitz", "amdgpu", "nouveau", "i915", "xe"].includes(read(`${path}/name`)))
      continue;
    for (const item of readdirSync(path).filter(n => /^temp\d+_input$/.test(n)))
      values.set(`${path}/${item}`, Number(read(`${path}/${item}`)) / 1000);
  }
  const gpu = Bun.spawnSync(["timeout", "2s", "nvidia-smi", "--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"]);
  if (gpu.exitCode === 0)
    for (const [i, t] of gpu.stdout.toString().trim().split("\n").entries())
      if (/^\d+$/.test(t)) values.set(`nvidia_gpu_${i}`, Number(t));
  return values;
}

function orderedArms(seed: number, ordinal: number): string[] {
  const arms = [...cfg.arms];
  const rng = 0x9E3779B97F4A7C15n;
  const wrap = (n: bigint) => BigInt.asUintN(64, n);
  let state = BigInt(cfg.session_seed ^ ordinal) ^ wrap(BigInt(seed) * rng);
  for (let i = arms.length - 1; i > 0; i--) {
    state = wrap(state + rng);
    let z = wrap((state ^ (state >> 30n)) * 0xBF58476D1CE4E5B9n);
    z = wrap((z ^ (z >> 27n)) * 0x94D049BB133111EBn);
    const j = Number((z ^ (z >> 31n)) % BigInt(i + 1));
    [arms[i], arms[j]] = [arms[j], arms[i]];
  }
  return arms;
}

let excursion: unknown;
const monitor = setInterval(() => {
  if (!excursion) {
    try {
      check();
    } catch (error) {
      excursion = error;
      log("environment", { label: "residency check failed", timestamp: new Date().toISOString(), error: String(error) });
    }
  }
}, 2000);
function guard() {
  if (excursion) throw excursion;
  check();
}
async function waitForBaseline() {
  await Bun.sleep(60_000);
  guard();
  const baseline = temperatures();
  log("run", { temperatures_c: Object.fromEntries(baseline) });
  return async () => {
    const start = performance.now();
    await Bun.sleep(15_000);
    for (;;) {
      guard();
      const current = temperatures();
      const elapsed = performance.now() - start;
      if ([...baseline].every(([id, base]) => (current.get(id) ?? Infinity) <= base + 3)) return elapsed;
      if (elapsed >= 120_000) throw new Error("temperature did not return within 3 °C of baseline in 120s");
      await Bun.sleep(Math.min(5000, 120_000 - elapsed));
    }
  };
}
async function launch(args: string[], description: object) {
  guard();
  const start = performance.now();
  const child = Bun.spawn([binary, "study", ...args], { stdout: "inherit", stderr: "inherit" });
  const exitCode = await child.exited;
  log("attempts", { ...description, exitCode, elapsed_ms: performance.now() - start });
  // Let an in-flight trial restore and close its scope before stopping the run.
  guard();
  return exitCode;
}

async function smoke(className: string, arm: string, seed: number, session: string, config: string, waitMs?: number) {
  return launch(["run", "--smoke", "--session", session, "--workload", className, "--arm", arm, "--seed", `${seed}`,
    "--trials-dir", dir, "--oracle-trace-dir", `${dir}/references`, "--config", config,
    "--llm-state", "LM Studio resident idle", "--llm-pid", pid],
    { kind: "trial", class: className, arm, seed, session, cooldown_ms: waitMs });
}

try {
  snapshot("before");
  log("run", { mode, binary, sha256: binarySha, config: cfg, pid });
  if (mode === "calibrate") {
    // The kernel continues at full duty after the quota. Measure total updates
    // over the fixed observation window, independent of any completion time.
    for (const [classIndex, className] of classes.entries()) {
      snapshot(`before ${className}`);
      for (const [index, seed] of cfg.seeds.slice(0, 2).entries()) {
        const arms = (classIndex + index) % 2 ? ["native", "static-low"] : ["static-low", "native"];
        for (const arm of arms) await smoke(className, arm, seed, "cal", cfgPath);
      }
      snapshot(`after ${className}`);
    }
  } else if (mode === "stress") {
    for (let index = 1; index <= 20; index++)
      await smoke("w3", "static-high", 1001, `stress-${index}`, cfgPath);
  } else if (scoredStress) {
    const seed = 2001;
    const exitCode = await launch(["record-traces", "--workload", "w1", "--seed", `${seed}`,
      "--config", cfgPath, "--out-dir", `${dir}/references`],
      { kind: "reference", class: "w1", seed });
    if (exitCode !== 0) throw new Error(`invalid W1 ${seed} Oracle reference`);
    for (const arm of ["reactive", "oracle"]) {
      snapshot(`before ${arm}`);
      for (let index = 1; index <= 20; index++)
        await smoke("w1", arm, seed, `stress-${arm}-${index}`, cfgPath);
      snapshot(`after ${arm}`);
    }
    const scopes = command("systemctl", "--user", "list-units", "harm-*.scope", "--no-legend", "--plain");
    log("run", { remaining_scopes: scopes, kernel: command("uname", "-r") });
    if (scopes) throw new Error(`HARM scopes remain after stress: ${scopes}`);
  } else if (scored) {
    const [, stage, session] = scored;
    const oracleDir = resolve(output, `prepare-a4-${session}`, "references");
    if (stage === "prepare") {
      const index = session === "A" ? 0 : 1;
      const n = cfg.matrix.seeds_per_session;
      const seeds = [...cfg.seeds.slice(index * n, (index + 1) * n), ...cfg.matrix.reserve_seeds[index]];
      const cooldown = await waitForBaseline();
      for (const seed of seeds) {
        const cooldown_ms = await cooldown();
        const status = await launch(["record-traces", "--workload", "w1", "--seed", `${seed}`,
          "--config", cfgPath, "--out-dir", oracleDir],
          { kind: "reference", class: "w1", seed, cooldown_ms });
        if (status !== 0) throw new Error(`invalid W1 ${seed} Oracle reference`);
      }
    } else {
      const status = await launch(["run", "--session", session, "--trials-dir", resolve(output, "sessions"),
        "--oracle-trace-dir", oracleDir, "--config", cfgPath,
        "--matrix-pilot-report", resolve(output, "pilot-w1-a4.json"),
        "--llm-state", "LM Studio resident-idle, no generation", "--llm-pid", pid],
        { kind: "session", session });
      if (status !== 0) throw new Error(`scored session ${session} exited ${status}`);
    }
  } else {
    // Use class batches as in the first pilot. Preserve the randomized arm order
    // within each matched block and capture a residency record at every boundary.
    const cooldown = await waitForBaseline();
    for (const [classIndex, className] of classes.entries()) {
      snapshot(`before ${className}`);
      const classStart = performance.now();
      for (const seed of cfg.seeds) {
        const waitMs = await cooldown();
        const exitCode = await launch(["record-traces", "--workload", className, "--seed", `${seed}`,
          "--config", cfgPath, "--out-dir", `${dir}/references`],
          { kind: "reference", class: className, seed, cooldown_ms: waitMs });
        if (exitCode !== 0) throw new Error(`invalid ${className} ${seed} Oracle reference`);
        for (const arm of orderedArms(seed, classIndex + 1))
          await smoke(className, arm, seed, `pilot-${className}`, cfgPath, await cooldown());
      }
      log("run", { class: className, elapsed_ms: performance.now() - classStart });
      snapshot(`after ${className}`);
    }
  }
  snapshot("after");
} catch (error) {
  log("run", { error: String(error), timestamp: new Date().toISOString() });
  throw error;
} finally {
  clearInterval(monitor);
}
