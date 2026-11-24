use super::*;
use gpu_probe::{Compute, Inference, Training};
use rustix::process::{getpriority_process, setpriority_process};
use std::{
    hint::black_box,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

const CHECK_BLOCK: u64 = 4096;

fn xorshift(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^ (state << 17)
}

pub(super) fn worker_seed(seed: u64, index: usize) -> u64 {
    (seed ^ (index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1)
}

fn set_nice(nice: i32) -> Result<()> {
    setpriority_process(None, nice)
        .map_err(|e| MatrixError::new(VoidReason::PriorityUnavailable, e))?;
    verify_nice(nice)
}

pub(super) fn verify_nice(nice: i32) -> Result<()> {
    let actual = getpriority_process(None)
        .map_err(|e| MatrixError::new(VoidReason::PriorityUnavailable, e))?;
    if actual != nice {
        return Err(MatrixError::new(
            VoidReason::PriorityUnavailable,
            format!("nice {actual}, expected {nice}"),
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct Gate {
    pub at: u64,
    pub end: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct WorkerReport {
    pub index: usize,
    pub seed_state: u64,
    pub completed_ns: Option<u64>,
    pub quota_checksum: Option<u64>,
    pub final_checksum: u64,
    pub observed_end_ns: u64,
}

fn burn(gate: Gate, seed: u64, index: usize, quota: Option<u64>) -> WorkerReport {
    let seed_state = worker_seed(seed, index);
    let mut state = seed_state;
    let mut remaining = quota;
    let mut quota_checksum = None;
    let mut completed_ns = None;
    let checksum = AtomicU64::new(seed_state);
    let published = black_box(&checksum);
    sleep_until(gate.at);
    let mut observed_end_ns = now();
    while observed_end_ns < gate.end {
        let count = remaining.unwrap_or(CHECK_BLOCK).min(CHECK_BLOCK);
        for _ in 0..count {
            state = xorshift(state);
        }
        published.store(state, Ordering::Relaxed);
        observed_end_ns = now();
        if let Some(left) = remaining.as_mut() {
            *left -= count;
            if *left == 0 {
                if observed_end_ns <= gate.end {
                    completed_ns = Some(observed_end_ns - gate.at);
                    quota_checksum = Some(state);
                }
                remaining = None;
            }
        }
    }
    WorkerReport {
        index,
        seed_state,
        completed_ns,
        quota_checksum,
        final_checksum: checksum.into_inner(),
        observed_end_ns,
    }
}

pub(super) struct Fillers {
    gates: Vec<mpsc::Sender<Gate>>,
    threads: Vec<JoinHandle<Result<WorkerReport>>>,
}

impl Fillers {
    fn prepare(threads: usize, nice: i32, seed: u64, quota: Option<u64>) -> Result<Self> {
        let mut fillers = Self {
            gates: Vec::new(),
            threads: Vec::new(),
        };
        let (ready, confirm) = mpsc::channel();
        for index in 0..threads {
            let (start, gate) = mpsc::channel();
            let ready = ready.clone();
            fillers.gates.push(start);
            fillers.threads.push(
                thread::Builder::new()
                    .name(format!("matrix-fill-{index}"))
                    .spawn(move || {
                        let priority = set_nice(nice);
                        ready.send(priority).ok();
                        let gate = gate
                            .recv()
                            .map_err(|e| MatrixError::new(VoidReason::Crash, e))?;
                        Ok(burn(gate, seed, index, quota))
                    })?,
            );
        }
        drop(ready);
        for _ in 0..threads {
            confirm
                .recv()
                .map_err(|e| MatrixError::new(VoidReason::Crash, e))??;
        }
        Ok(fillers)
    }

    pub fn manager(threads: usize, seed: u64) -> Result<Self> {
        Self::prepare(threads, 19, seed, None)
    }

    pub fn start(&mut self, gate: Gate) -> Result<()> {
        for tx in self.gates.drain(..) {
            tx.send(gate)
                .map_err(|e| MatrixError::new(VoidReason::Crash, e))?;
        }
        Ok(())
    }

    pub fn join(mut self) -> Result<Vec<WorkerReport>> {
        self.threads.drain(..).map(join).collect()
    }
}

impl Drop for Fillers {
    fn drop(&mut self) {
        self.gates.clear();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

pub fn run_fixed_worker(seed: u64, count: usize, quota: u64) -> Result<()> {
    let mut workers = Fillers::prepare(count, 10, seed, Some(quota))?;
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "ready")?;
    stdout.flush()?;
    let mut gate = [0; 16];
    io::stdin().lock().read_exact(&mut gate)?;
    let at = u64::from_le_bytes(gate[..8].try_into().expect("8 bytes"));
    let end = u64::from_le_bytes(gate[8..].try_into().expect("8 bytes"));
    workers.start(Gate { at, end })?;
    serde_json::to_writer(&mut stdout, &workers.join()?)?;
    writeln!(stdout)?;
    Ok(())
}

pub(super) struct CpuChild {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl CpuChild {
    pub fn spawn(seed: u64, count: usize, quota: u64) -> Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("__matrix-cpu")
            .arg(seed.to_string())
            .arg(count.to_string())
            .arg(quota.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(Self {
            child,
            input,
            output,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn wait_ready(&mut self) -> Result<()> {
        let mut line = String::new();
        self.output.read_line(&mut line)?;
        if line.trim_end() != "ready" {
            return Err(MatrixError::new(
                VoidReason::PriorityUnavailable,
                "fixed CPU workers did not confirm setup",
            ));
        }
        Ok(())
    }

    pub fn start(&mut self, gate: Gate) -> Result<()> {
        self.input.write_all(&gate.at.to_le_bytes())?;
        self.input.write_all(&gate.end.to_le_bytes())?;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<WorkerReport>> {
        let mut line = String::new();
        self.output.read_line(&mut line)?;
        let status = self.child.wait()?;
        if !status.success() {
            return Err(MatrixError::new(
                VoidReason::Crash,
                format!("fixed CPU worker exited {status}"),
            ));
        }
        Ok(serde_json::from_str(&line)?)
    }
}

impl Drop for CpuChild {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None) | Err(_)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DropReason {
    InFlight,
    HostDidNotSubmitBeforeDeadline,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(super) struct Event {
    pub phase: usize,
    pub request_id: Option<u64>,
    pub release_ns: u64,
    pub deadline_ns: Option<u64>,
    pub submit_ns: Option<u64>,
    pub checked_ns: Option<u64>,
    pub observed_ns: u64,
    pub output: Option<f32>,
    pub drop_reason: Option<DropReason>,
}

impl Event {
    pub fn latency(self) -> Option<u128> {
        self.checked_ns?
            .checked_sub(self.release_ns)
            .map(u128::from)
    }

    pub fn miss(self) -> bool {
        self.deadline_ns
            .is_some_and(|due| self.checked_ns.is_none_or(|checked| checked > due))
    }
}

pub(super) type Events = Arc<Mutex<Vec<Event>>>;

enum ForegroundJob {
    Ping,
    Inference(Inference),
    Training(Training),
}

fn seeded_input(state: &mut u64, count: u32) -> Vec<f32> {
    (0..count)
        .map(|_| {
            *state = xorshift(*state);
            ((*state >> 40) as f32 / 16_777_216.0) * 0.1 - 0.05
        })
        .collect()
}

fn prepare_gpu(
    class: MatrixClass,
    matrix: &MatrixConfig,
    seed: u64,
) -> Result<(ForegroundJob, Option<String>)> {
    let mut rng = seed.max(1);
    let compute = Compute::open()?;
    let adapter = compute.adapter();
    let job = if class == MatrixClass::W3 {
        let cfg = &matrix.workloads.w3;
        let [m, k, n] = cfg.matmul_mkn;
        let x = seeded_input(&mut rng, m * k);
        let w = seeded_input(&mut rng, k * n);
        let job = Inference::new(compute, cfg.matmul_mkn, &x, &w, cfg.dispatches_per_batch);
        for _ in 0..cfg.warmup_batches {
            job.batch()?;
        }
        ForegroundJob::Inference(job)
    } else {
        let cfg = &matrix.workloads.w4;
        let [m, k, n] = [cfg.batch_size, cfg.inputs, cfg.outputs];
        let x = seeded_input(&mut rng, m * k);
        let w = seeded_input(&mut rng, k * n);
        let y = seeded_input(&mut rng, m * n);
        let lr = cfg.learning_rate as f32;
        let job = Training::new(compute, [m, k, n], &x, &w, &y, lr);
        job.check_first_step(&x, &w, &y, lr)?;
        for _ in 1..cfg.warmup_steps {
            job.step()?;
        }
        job.reset(&w)?;
        ForegroundJob::Training(job)
    };
    // W3 inputs are immutable and every dispatch overwrites its output.
    Ok((job, Some(adapter)))
}

pub(super) struct Foreground {
    pub events: Events,
    pub adapter: Option<String>,
    start: Option<mpsc::Sender<u64>>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Foreground {
    pub fn prepare(
        class: MatrixClass,
        matrix: &MatrixConfig,
        phases: Phases,
        seed: u64,
    ) -> Result<Self> {
        let cpu = matches!(class, MatrixClass::W1 | MatrixClass::W2);
        let events = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&events);
        let (start, gate) = mpsc::channel();
        let (ready, setup) = mpsc::channel();
        let matrix = matrix.clone();
        let thread = thread::Builder::new()
            .name("matrix-foreground".into())
            .spawn(move || {
                // Resize and touch all pages before the gate. A growth past this reservation
                // is still lossless.
                if cpu {
                    let mut output = output.lock().expect("events lock");
                    output.resize(
                        2_000_000,
                        Event {
                            phase: 0,
                            request_id: None,
                            release_ns: 0,
                            deadline_ns: None,
                            submit_ns: None,
                            checked_ns: None,
                            observed_ns: 0,
                            output: None,
                            drop_reason: None,
                        },
                    );
                    black_box(output.as_slice());
                    output.clear();
                }
                let prepared = if cpu {
                    Ok((ForegroundJob::Ping, None))
                } else {
                    prepare_gpu(class, &matrix, seed)
                };
                let (job, adapter) = match prepared {
                    Ok(pair) => pair,
                    Err(e) => {
                        ready.send(Err(e)).ok();
                        return Ok(());
                    }
                };
                match job {
                    ForegroundJob::Ping => {
                        let (mut client, mut server) = UnixStream::pair()?;
                        client.set_read_timeout(Some(Duration::from_secs(2)))?;
                        client.set_write_timeout(Some(Duration::from_secs(2)))?;
                        let (echo_ready, wait_echo) = mpsc::channel();
                        let echo =
                            thread::Builder::new()
                                .name("matrix-echo".into())
                                .spawn(move || {
                                    echo_ready.send(()).ok();
                                    let mut byte = [0];
                                    while server.read_exact(&mut byte).is_ok() {
                                        server.write_all(&byte)?;
                                    }
                                    Ok(())
                                })?;
                        wait_echo
                            .recv()
                            .map_err(|e| MatrixError::new(VoidReason::Crash, e))?;
                        ready.send(Ok(adapter)).ok();
                        let result = gate.recv().ok().map_or(Ok(()), |at| {
                            run_ping(&mut client, class, phases, at, &output)
                        });
                        drop(client);
                        let echo_result = join(echo);
                        result.and(echo_result)
                    }
                    ForegroundJob::Inference(job) => {
                        ready.send(Ok(adapter)).ok();
                        gate.recv().ok().map_or(Ok(()), |at| {
                            run_inference(&job, &matrix, phases, at, &output)
                        })
                    }
                    ForegroundJob::Training(job) => {
                        ready.send(Ok(adapter)).ok();
                        gate.recv()
                            .ok()
                            .map_or(Ok(()), |at| run_training(&job, phases, at, &output))
                    }
                }
            })?;
        let mut foreground = Self {
            events,
            adapter: None,
            start: Some(start),
            thread: Some(thread),
        };
        foreground.adapter = setup
            .recv()
            .map_err(|e| MatrixError::new(VoidReason::Crash, e))??;
        Ok(foreground)
    }

    pub fn start(&mut self, at: u64) -> Result<()> {
        self.start
            .take()
            .expect("start once")
            .send(at)
            .map_err(|e| MatrixError::new(VoidReason::Crash, e))
    }

    pub fn finish(&mut self) -> Result<()> {
        join(self.thread.take().expect("join once"))
    }
}

impl Drop for Foreground {
    fn drop(&mut self) {
        self.start.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn join<T>(thread: JoinHandle<Result<T>>) -> Result<T> {
    thread
        .join()
        .map_err(|_| MatrixError::new(VoidReason::Crash, "workload thread panicked"))?
}

fn publish(events: &Events, event: Event) {
    events.lock().expect("events lock").push(event);
}

fn run_ping(
    client: &mut UnixStream,
    class: MatrixClass,
    phases: Phases,
    at: u64,
    events: &Events,
) -> Result<()> {
    sleep_until(at);
    loop {
        let release = now() - at;
        let Some(phase) = phases.phase(release) else {
            return Ok(());
        };
        if !class.active(phase) {
            sleep_until(at + phases.end(phase));
            continue;
        }
        client.write_all(&[1])?;
        let mut ack = [0];
        client.read_exact(&mut ack)?;
        let checked = now() - at;
        if ack != [1] {
            return Err(MatrixError::new(
                VoidReason::Crash,
                "wrong UnixStream echo byte",
            ));
        }
        publish(
            events,
            Event {
                phase,
                request_id: None,
                release_ns: release,
                deadline_ns: None,
                submit_ns: Some(release),
                checked_ns: Some(checked),
                observed_ns: checked,
                output: None,
                drop_reason: None,
            },
        );
    }
}

pub(super) fn inference_slots(
    phases: Phases,
    phase: usize,
    period_ns: u64,
) -> impl Iterator<Item = u64> {
    (phases.start(phase)..phases.end(phase)).step_by(period_ns as usize)
}

fn drop_reason(
    release: u64,
    deadline: u64,
    phase_end: u64,
    now: u64,
    previous_checked: Option<u64>,
) -> Option<DropReason> {
    if previous_checked.is_some_and(|checked| checked > release) {
        Some(DropReason::InFlight)
    } else if now >= deadline || now >= phase_end {
        Some(DropReason::HostDidNotSubmitBeforeDeadline)
    } else {
        None
    }
}

fn run_inference(
    job: &Inference,
    matrix: &MatrixConfig,
    phases: Phases,
    at: u64,
    events: &Events,
) -> Result<()> {
    let cfg = &matrix.workloads.w3;
    let mut previous_checked = None;
    let slots = PROTECTED.into_iter().flat_map(|phase| {
        inference_slots(phases, phase, cfg.batch_period_ms * MS)
            .map(move |release| (phase, release))
    });
    for (id, (phase, release)) in slots.enumerate() {
        let deadline = release + cfg.batch_deadline_ms * MS;
        sleep_until(at + release);
        let observed = now() - at;
        let reason = drop_reason(
            release,
            deadline,
            phases.end(phase),
            observed,
            previous_checked,
        );
        let (submit, checked, output, observed) = if reason.is_some() {
            (None, None, None, observed)
        } else {
            let output = job.batch()?;
            let checked = now() - at;
            previous_checked = Some(checked);
            (Some(observed), Some(checked), Some(output), checked)
        };
        publish(
            events,
            Event {
                phase,
                request_id: Some(id as u64),
                release_ns: release,
                deadline_ns: Some(deadline),
                submit_ns: submit,
                checked_ns: checked,
                observed_ns: observed,
                output,
                drop_reason: reason,
            },
        );
    }
    Ok(())
}

fn run_training(job: &Training, phases: Phases, at: u64, events: &Events) -> Result<()> {
    for phase in PROTECTED {
        sleep_until(at + phases.start(phase));
        loop {
            let release = now() - at;
            if release >= phases.end(phase) {
                break;
            }
            let output = job.step()?;
            let checked = now() - at;
            publish(
                events,
                Event {
                    phase,
                    request_id: None,
                    release_ns: release,
                    deadline_ns: None,
                    submit_ns: Some(release),
                    checked_ns: Some(checked),
                    observed_ns: checked,
                    output: Some(output),
                    drop_reason: None,
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_uses_wrapping_one_based_indices_and_replaces_zero() {
        assert_eq!(worker_seed(0, 0), 0x9e37_79b9_7f4a_7c15);
        assert_eq!(worker_seed(1001, 1), 0x3c6e_f372_fe94_fbc3);
        assert_eq!(worker_seed(0x9e37_79b9_7f4a_7c15, 0), 1);
        assert_eq!(xorshift(1), 1_082_269_761);
    }

    #[test]
    fn inference_schedule_and_deadline_accounting() {
        let phases = Phases::new([4., 2., 4., 2.]);
        let slots: Vec<_> = inference_slots(phases, 1, 10 * MS).collect();
        assert_eq!(slots.len(), 200);
        assert_eq!((slots[0], slots[199] + 10 * MS), (4 * SECOND, 6 * SECOND));
        let release = slots[0];
        let deadline = release + 10 * MS;
        let event = Event {
            phase: 1,
            request_id: Some(0),
            release_ns: release,
            deadline_ns: Some(deadline),
            submit_ns: Some(deadline - MS),
            checked_ns: Some(deadline),
            observed_ns: deadline,
            output: Some(0.0),
            drop_reason: None,
        };
        assert!(!event.miss());
        assert!(
            Event {
                checked_ns: Some(deadline + 1),
                ..event
            }
            .miss()
        );
        let dropped = Event {
            submit_ns: None,
            checked_ns: None,
            ..event
        };
        assert!(dropped.miss());
        assert_eq!(dropped.latency(), None);
        assert_eq!(
            drop_reason(release, deadline, phases.end(1), deadline, None),
            Some(DropReason::HostDidNotSubmitBeforeDeadline)
        );
        assert_eq!(
            drop_reason(
                release,
                deadline,
                phases.end(1),
                release + 1,
                Some(release + 1)
            ),
            Some(DropReason::InFlight)
        );
        assert_eq!(
            drop_reason(
                release,
                deadline,
                phases.end(1),
                deadline - 1,
                Some(release)
            ),
            None
        );
    }
}
