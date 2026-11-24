use crate::{
    ActionError, Seconds, Source,
    predict::{MetaAction, Observation, ObservationWindow, OraclePredictor, predict_with_budget},
    validate_cpu_weight,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlObservation {
    pub workload: String,
    pub psi_cpu_some_avg10: Option<f64>,
    pub current_weight: u32,
    pub monotonic_seconds: f64,
    pub source: Source,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlAction {
    pub workload: String,
    pub cgroup: PathBuf,
    pub cpu_weight: u32,
    pub expires_at_seconds: f64,
    pub source: Source,
}
pub trait Controller: Send {
    fn decide(
        &mut self,
        observation: &ControlObservation,
        workload_cgroup: &Path,
    ) -> Option<ControlAction>;
}
#[derive(Default)]
pub struct NativeController;
impl Controller for NativeController {
    fn decide(&mut self, _: &ControlObservation, _: &Path) -> Option<ControlAction> {
        None
    }
}
pub struct StaticController {
    pub weight: u32,
}
impl Controller for StaticController {
    fn decide(&mut self, o: &ControlObservation, path: &Path) -> Option<ControlAction> {
        Some(ControlAction {
            workload: o.workload.clone(),
            cgroup: path.into(),
            cpu_weight: self.weight,
            expires_at_seconds: f64::INFINITY,
            source: Source::Assumed,
        })
    }
}
pub struct ReactiveController {
    pub low_pressure_weight: u32,
    pub high_pressure_weight: u32,
    pub threshold: f64,
}
impl Controller for ReactiveController {
    fn decide(&mut self, o: &ControlObservation, path: &Path) -> Option<ControlAction> {
        let pressure = o.psi_cpu_some_avg10?;
        Some(ControlAction {
            workload: o.workload.clone(),
            cgroup: path.into(),
            cpu_weight: if pressure >= self.threshold {
                self.high_pressure_weight
            } else {
                self.low_pressure_weight
            },
            expires_at_seconds: o.monotonic_seconds + 1.0,
            source: o.source,
        })
    }
}
pub struct OracleController {
    predictor: OraclePredictor,
    target: String,
    model_id: String,
}
impl OracleController {
    pub fn from_trace_file(path: &Path, target: String) -> Result<Self, String> {
        Ok(Self {
            predictor: OraclePredictor::from_pre_recorded_file(path)
                .map_err(|e| format!("{e:?}"))?,
            target,
            model_id: "oracle-trace".into(),
        })
    }
}
impl Controller for OracleController {
    fn decide(&mut self, o: &ControlObservation, path: &Path) -> Option<ControlAction> {
        let obs = Observation {
            target: self.target.clone(),
            observed_at_seconds: o.monotonic_seconds,
            value: o.psi_cpu_some_avg10?,
            source: Source::Observed,
        };
        let window = ObservationWindow {
            observations: vec![obs],
        };
        let request = MetaAction {
            model_id: self.model_id.clone(),
            device: "offline".into(),
            budget: Seconds::new(1.0)?,
            horizon: Seconds::new(1.0)?,
            target: self.target.clone(),
        };
        let prediction = predict_with_budget(&self.predictor, &window, &request).ok()?;
        Some(ControlAction {
            workload: o.workload.clone(),
            cgroup: path.into(),
            cpu_weight: if prediction.predicted_value >= self.threshold() {
                1000
            } else {
                100
            },
            expires_at_seconds: prediction.expires_at_seconds,
            source: prediction.source,
        })
    }
}
impl OracleController {
    fn threshold(&self) -> f64 {
        0.2
    }
}

/// Study Oracle for the Phase 3 upper-bound experiment. Its future knowledge comes *only* from a
/// pre-recorded [`FutureTrace`] file; its present knowledge is only `observation.monotonic_seconds`.
/// It never reads live PSI, load, or the current weight to infer the future — so it cannot leak runtime
/// state into its decision. Ahead of each protected phase (a high-duty label scheduled in the trace) it
/// lowers background weight; during unprotected phases it raises it back.
pub struct FuturePhaseController {
    trace: crate::trace::FutureTrace,
    horizon_seconds: f64,
    protect_phases: std::collections::HashSet<String>,
    protected_weight: u32,
    free_weight: u32,
}

impl FuturePhaseController {
    pub fn from_trace_file(
        path: &Path,
        horizon_seconds: f64,
        protect_phases: Vec<String>,
        protected_weight: u32,
        free_weight: u32,
    ) -> Result<Self, String> {
        Ok(Self {
            trace: crate::trace::FutureTrace::from_file(path)?,
            horizon_seconds,
            protect_phases: protect_phases.into_iter().collect(),
            protected_weight,
            free_weight,
        })
    }

    /// The weight the controller wants at monotonic time `now`. It protects background when either the
    /// phase active *now* or the phase active at `now + horizon` is a protected (high-duty) label — so it
    /// lowers weight ahead of an upcoming transition and keeps protecting through the final recorded phase
    /// even once the horizon runs past the end of the trace. Outside any recorded phase there is no known
    /// future, so it falls back to the free weight rather than guessing. Uses only file data + present clock.
    pub fn desired_weight(&self, now: f64) -> u32 {
        let protected = |t: f64| {
            self.trace
                .phase_at(t)
                .is_some_and(|p| self.protect_phases.contains(&p.phase))
        };
        if protected(now + self.horizon_seconds) || protected(now) {
            self.protected_weight
        } else {
            self.free_weight
        }
    }
}

impl Controller for FuturePhaseController {
    fn decide(&mut self, o: &ControlObservation, path: &Path) -> Option<ControlAction> {
        // Only the present monotonic clock is read from the observation; no live metric influences it.
        let weight = self.desired_weight(o.monotonic_seconds);
        Some(ControlAction {
            workload: o.workload.clone(),
            cgroup: path.into(),
            cpu_weight: weight,
            expires_at_seconds: f64::INFINITY,
            source: Source::Assumed, // study-only; provenance is not Oracle so the guard can apply it
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlError {
    Unavailable(String),
    PathNotWhitelisted(PathBuf),
    OracleNotProductionSafe,
    InvalidWeight,
    Expired,
    RateLimited,
    ProtectedMinimum,
    Io(String),
    ReadbackMismatch { expected: u32, actual: u32 },
}
impl From<ActionError> for ControlError {
    fn from(_: ActionError) -> Self {
        Self::InvalidWeight
    }
}
fn classify_cgroup_io(error: &io::Error, context: String) -> ControlError {
    let reason = format!("{context}: {error}");
    match error.kind() {
        io::ErrorKind::PermissionDenied
        | io::ErrorKind::NotFound
        | io::ErrorKind::ReadOnlyFilesystem => ControlError::Unavailable(reason),
        _ => ControlError::Io(reason),
    }
}
pub trait CgroupIo {
    fn read_weight(&self, path: &Path) -> io::Result<u32>;
    fn write_weight(&self, path: &Path, weight: u32) -> io::Result<()>;
}
#[derive(Clone, Copy, Default)]
pub struct FileCgroupIo;
impl CgroupIo for FileCgroupIo {
    fn read_weight(&self, path: &Path) -> io::Result<u32> {
        fs::read_to_string(path.join("cpu.weight"))?
            .trim()
            .parse()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    fn write_weight(&self, path: &Path, weight: u32) -> io::Result<()> {
        fs::write(path.join("cpu.weight"), weight.to_string())
    }
}
#[derive(Default)]
pub struct PathWhitelist {
    paths: HashSet<PathBuf>,
}
impl PathWhitelist {
    pub fn mark_created(&mut self, path: PathBuf) {
        self.paths.insert(path);
    }
    pub fn grant(&mut self, path: PathBuf) {
        self.paths.insert(path);
    }
    pub fn permits(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }
}
#[derive(Clone, Debug)]
pub struct ControlGuards {
    pub max_delta: u32,
    pub min_background_weight: u32,
    pub max_frequency: Duration,
}
impl Default for ControlGuards {
    fn default() -> Self {
        Self {
            max_delta: 500,
            min_background_weight: 100,
            max_frequency: Duration::from_millis(100),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub dry_run: bool,
    pub is_background: bool,
    pub now_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditRecord {
    pub timestamp_unix_seconds: f64,
    pub monotonic_seconds: f64,
    pub event: String,
    pub path: String,
    pub requested: Option<u32>,
    pub before: Option<u32>,
    pub after: Option<u32>,
    pub readback: Option<u32>,
    pub result: String,
    pub reason: String,
}
fn audit(path: &Path, record: &AuditRecord) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut f, record).map_err(io::Error::other)?;
    f.write_all(b"\n")
}
#[allow(clippy::too_many_arguments)]
fn audit_record(
    event: &str,
    path: &Path,
    requested: Option<u32>,
    before: Option<u32>,
    after: Option<u32>,
    readback: Option<u32>,
    result: &str,
    reason: &str,
) -> AuditRecord {
    AuditRecord {
        timestamp_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64(),
        monotonic_seconds: crate::monotonic_seconds(),
        event: event.into(),
        path: path.display().to_string(),
        requested,
        before,
        after,
        readback,
        result: result.into(),
        reason: reason.into(),
    }
}
fn rollback_status(result: io::Result<u32>, before: u32) -> (Option<u32>, &'static str) {
    match result {
        Ok(value) if value == before => (Some(value), "rolled_back"),
        Ok(value) => (Some(value), "rollback_mismatch"),
        Err(_) => (None, "rollback_failed"),
    }
}

pub struct WeightController<I: CgroupIo = FileCgroupIo> {
    pub io: I,
    pub whitelist: PathWhitelist,
    pub guards: ControlGuards,
    pub audit_path: PathBuf,
    pub last_change: Option<Instant>,
    originals: HashMap<PathBuf, u32>,
}
impl<I: CgroupIo> WeightController<I> {
    pub fn new(
        io: I,
        whitelist: PathWhitelist,
        guards: ControlGuards,
        audit_path: PathBuf,
    ) -> Self {
        Self {
            io,
            whitelist,
            guards,
            audit_path,
            last_change: None,
            originals: HashMap::new(),
        }
    }
    pub fn apply(
        &mut self,
        action: &ControlAction,
        options: ApplyOptions,
    ) -> Result<ApplyResult, ControlError> {
        let path = &action.cgroup;
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)
            .map_err(|e| {
                ControlError::Io(format!("audit preflight failed; refusing action: {e}"))
            })?;
        let refuse = |reason: &str| {
            audit(
                &self.audit_path,
                &audit_record(
                    "refused",
                    path,
                    Some(action.cpu_weight),
                    None,
                    None,
                    None,
                    "refused",
                    reason,
                ),
            )
            .map_err(|e| ControlError::Io(format!("audit refused action: {e}")))
        };
        if !self.whitelist.permits(path) {
            refuse("path not in HARM-created or explicitly granted whitelist")?;
            return Err(ControlError::PathNotWhitelisted(path.clone()));
        }
        if action.source == Source::Oracle {
            refuse("oracle provenance is offline-only")?;
            return Err(ControlError::OracleNotProductionSafe);
        }
        if !options.now_seconds.is_finite()
            || action.expires_at_seconds.is_nan()
            || options.now_seconds > action.expires_at_seconds
        {
            refuse("action expired or clock invalid")?;
            return Err(ControlError::Expired);
        }
        let requested = match validate_cpu_weight(u64::from(action.cpu_weight)) {
            Ok(v) => v as u32,
            Err(_) => {
                refuse("weight outside 1..=10000")?;
                return Err(ControlError::InvalidWeight);
            }
        };
        if self
            .last_change
            .is_some_and(|t| t.elapsed() < self.guards.max_frequency)
        {
            refuse("maximum control frequency")?;
            return Err(ControlError::RateLimited);
        }
        if options.is_background && requested < self.guards.min_background_weight {
            refuse("minimum background weight protection")?;
            return Err(ControlError::ProtectedMinimum);
        }
        let before = match self.io.read_weight(path) {
            Ok(v) => v,
            Err(e) => {
                refuse(&format!("read current cpu.weight failed: {e}"))?;
                return Err(classify_cgroup_io(&e, "read cpu.weight".into()));
            }
        };
        let low = before
            .saturating_sub(self.guards.max_delta)
            .max(if options.is_background {
                self.guards.min_background_weight
            } else {
                1
            });
        let high = before.saturating_add(self.guards.max_delta).min(10_000);
        let intended = requested.clamp(low, high);
        if options.dry_run {
            audit(
                &self.audit_path,
                &audit_record(
                    "dry_run",
                    path,
                    Some(intended),
                    Some(before),
                    Some(before),
                    Some(before),
                    "not_applied",
                    "dry run; no cgroup write",
                ),
            )
            .map_err(|e| ControlError::Io(e.to_string()))?;
            return Ok(ApplyResult {
                before,
                after: before,
                readback: before,
                dry_run: true,
            });
        }
        if intended != before {
            self.originals.entry(path.clone()).or_insert(before);
        }
        if let Err(error) = self.io.write_weight(path, intended) {
            let rollback = self
                .io
                .write_weight(path, before)
                .and_then(|_| self.io.read_weight(path));
            let (after, status) = rollback_status(rollback, before);
            let reason = format!("write failed: {error}; rollback readback={after:?}");
            audit(
                &self.audit_path,
                &audit_record(
                    "rollback",
                    path,
                    Some(intended),
                    Some(before),
                    after,
                    None,
                    status,
                    &reason,
                ),
            )
            .map_err(|e| ControlError::Io(e.to_string()))?;
            return Err(classify_cgroup_io(&error, reason));
        }
        let readback = match self.io.read_weight(path) {
            Ok(value) => value,
            Err(error) => {
                let rollback = self
                    .io
                    .write_weight(path, before)
                    .and_then(|_| self.io.read_weight(path));
                let (after, status) = rollback_status(rollback, before);
                let reason = format!("readback failed: {error}; rollback readback={after:?}");
                audit(
                    &self.audit_path,
                    &audit_record(
                        "rollback",
                        path,
                        Some(intended),
                        Some(before),
                        after,
                        None,
                        status,
                        &reason,
                    ),
                )
                .map_err(|e| ControlError::Io(e.to_string()))?;
                return Err(classify_cgroup_io(&error, reason));
            }
        };
        if readback != intended {
            let rollback = self
                .io
                .write_weight(path, before)
                .and_then(|_| self.io.read_weight(path));
            let (after, status) = rollback_status(rollback, before);
            let reason = format!("readback mismatch; rollback readback={after:?}");
            audit(
                &self.audit_path,
                &audit_record(
                    "rollback",
                    path,
                    Some(intended),
                    Some(before),
                    after,
                    Some(readback),
                    status,
                    &reason,
                ),
            )
            .map_err(|e| ControlError::Io(e.to_string()))?;
            return Err(ControlError::ReadbackMismatch {
                expected: intended,
                actual: readback,
            });
        }
        if let Err(error) = audit(
            &self.audit_path,
            &audit_record(
                "apply",
                path,
                Some(intended),
                Some(before),
                Some(intended),
                Some(readback),
                "applied",
                "write verified by readback",
            ),
        ) {
            let rollback = self
                .io
                .write_weight(path, before)
                .and_then(|_| self.io.read_weight(path));
            return Err(ControlError::Io(format!(
                "audit applied action failed ({error}); rollback={rollback:?}"
            )));
        }
        self.last_change = Some(Instant::now());
        Ok(ApplyResult {
            before,
            after: intended,
            readback,
            dry_run: false,
        })
    }
    pub fn restore_all(&mut self) -> Result<(), ControlError> {
        let entries: Vec<_> = self
            .originals
            .iter()
            .map(|(path, original)| (path.clone(), *original))
            .collect();
        for (path, original) in &entries {
            // Read the value currently in effect so the audit trail records what we
            // are restoring *from* (normally the applied weight), not just where we
            // end up. A restore that omits `before` is non-reconstructable.
            let current = self.io.read_weight(path).ok();
            self.io
                .write_weight(path, *original)
                .map_err(|e| classify_cgroup_io(&e, format!("restore {}", path.display())))?;
            let readback = self.io.read_weight(path).map_err(|e| {
                classify_cgroup_io(&e, format!("restore readback {}", path.display()))
            })?;
            audit(
                &self.audit_path,
                &audit_record(
                    "restore",
                    path,
                    Some(*original),
                    current,
                    Some(*original),
                    Some(readback),
                    if readback == *original {
                        "restored"
                    } else {
                        "restore_mismatch"
                    },
                    "restore original cpu.weight",
                ),
            )
            .map_err(|e| ControlError::Io(e.to_string()))?;
            if readback != *original {
                return Err(ControlError::ReadbackMismatch {
                    expected: *original,
                    actual: readback,
                });
            }
        }
        self.originals.clear();
        Ok(())
    }
}
impl<I: CgroupIo> Drop for WeightController<I> {
    fn drop(&mut self) {
        for (path, original) in &self.originals {
            let current = self.io.read_weight(path).ok();
            let result = self
                .io
                .write_weight(path, *original)
                .and_then(|_| self.io.read_weight(path));
            let (readback, status) = match result {
                Ok(value) => (
                    Some(value),
                    if value == *original {
                        "restored"
                    } else {
                        "restore_mismatch"
                    },
                ),
                Err(_) => (None, "restore_failed"),
            };
            let _ = audit(
                &self.audit_path,
                &audit_record(
                    "restore",
                    path,
                    Some(*original),
                    current,
                    Some(*original),
                    readback,
                    status,
                    "WeightController Drop restoration",
                ),
            );
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyResult {
    pub before: u32,
    pub after: u32,
    pub readback: u32,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct DelegatedScope {
    pub unit: String,
    pub root: PathBuf,
    pub manager: PathBuf,
    pub workload: PathBuf,
    pub pid: u32,
}
impl DelegatedScope {
    pub fn discover_and_delegate(unit: &str) -> Result<Self, ControlError> {
        let cgroup = fs::read_to_string("/proc/self/cgroup")
            .map_err(|e| ControlError::Unavailable(e.to_string()))?;
        let relative = cgroup
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .ok_or_else(|| ControlError::Unavailable("unified cgroup entry absent".into()))?;
        let root = PathBuf::from("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        let uid = current_uid()
            .ok_or_else(|| ControlError::Unavailable("cannot establish uid".into()))?;
        let app_slice = PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice"
        ));
        let basename = root
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or_default();
        let expected_scope = format!("{unit}.scope");
        if !unit.starts_with("harm-")
            || !root.starts_with(&app_slice)
            || (basename != unit && basename != expected_scope)
        {
            return Err(ControlError::Unavailable(format!(
                "refusing cgroup outside HARM-created user app scope: {}",
                root.display()
            )));
        }
        let manager = root.join("harm-manager");
        let workload = root.join("harm-workload");
        fs::create_dir(&manager)
            .map_err(|e| ControlError::Unavailable(format!("cannot create manager child: {e}")))?;
        let scope_processes = fs::read_to_string(root.join("cgroup.procs"))
            .map_err(|e| ControlError::Unavailable(format!("read delegated scope PIDs: {e}")))?;
        let scope_pids = scope_processes
            .lines()
            .map(|line| {
                line.trim()
                    .parse::<u32>()
                    .map_err(|e| ControlError::Unavailable(format!("invalid scope PID: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !scope_pids.contains(&std::process::id()) {
            return Err(ControlError::Unavailable(
                "HARM PID is not in the expected transient scope".into(),
            ));
        }
        for pid in scope_pids {
            fs::write(manager.join("cgroup.procs"), pid.to_string()).map_err(|e| {
                ControlError::Unavailable(format!("move scope PID {pid} into manager child: {e}"))
            })?;
        }
        let manager_processes = fs::read_to_string(manager.join("cgroup.procs"))
            .map_err(|e| ControlError::Unavailable(format!("verify manager PIDs: {e}")))?;
        if !manager_processes
            .lines()
            .any(|line| line.trim().parse::<u32>().ok() == Some(std::process::id()))
        {
            return Err(ControlError::Unavailable(
                "HARM PID did not move into manager child".into(),
            ));
        }
        let remaining = fs::read_to_string(root.join("cgroup.procs")).map_err(|e| {
            ControlError::Unavailable(format!("verify no-internal-process rule: {e}"))
        })?;
        if !remaining.trim().is_empty() {
            return Err(ControlError::Unavailable(
                "scope still contains internal processes before enabling cpu".into(),
            ));
        }
        fs::write(root.join("cgroup.subtree_control"), "+cpu")
            .map_err(|e| ControlError::Unavailable(format!("cannot enable cpu controller: {e}")))?;
        fs::create_dir(&workload).map_err(|e| {
            ControlError::Unavailable(format!("cannot create workload cgroup: {e}"))
        })?;
        Ok(Self {
            unit: unit.into(),
            root,
            manager,
            workload,
            pid: std::process::id(),
        })
    }
    pub fn attach_pid(&self, pid: u32) -> Result<(), ControlError> {
        let path = self.workload.join("cgroup.procs");
        fs::write(&path, pid.to_string())
            .map_err(|e| classify_cgroup_io(&e, format!("attach PID {pid}")))?;
        let members = fs::read_to_string(&path)
            .map_err(|e| classify_cgroup_io(&e, format!("verify PID {pid} placement")))?;
        if !members
            .lines()
            .any(|member| member.trim().parse::<u32>().ok() == Some(pid))
        {
            return Err(ControlError::Io(format!(
                "PID {pid} did not appear in {} after migration",
                path.display()
            )));
        }
        Ok(())
    }
    pub fn remove_workload(&self) -> Result<(), ControlError> {
        fs::remove_dir(&self.workload)
            .map_err(|e| ControlError::Io(format!("remove workload cgroup: {e}")))
    }
}

pub fn launch_delegated(
    executable: &Path,
    unit: &str,
    args: &[String],
) -> Result<(), ControlError> {
    let exe = executable
        .to_str()
        .ok_or_else(|| ControlError::Unavailable("non-UTF8 executable path".into()))?;
    let status = Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "-q",
            "-p",
            "Delegate=yes",
            "--unit",
            unit,
            "--",
            exe,
        ])
        .args(args)
        .status()
        .map_err(|e| {
            ControlError::Unavailable(format!("systemd-run --user --scope failed to start: {e}"))
        })?;
    let scope_path = expected_scope_path(unit).ok_or_else(|| {
        ControlError::Unavailable("cannot verify transient scope cleanup path".into())
    })?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while scope_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let lingering_scope = scope_path.exists().then_some(scope_path);
    if !status.success() {
        return Err(ControlError::Unavailable(format!(
            "systemd-run exited with {status}; lingering scope={lingering_scope:?}"
        )));
    }
    if let Some(path) = lingering_scope {
        return Err(ControlError::Unavailable(format!(
            "transient experiment scope remained after systemd-run exited: {}",
            path.display()
        )));
    }
    Ok(())
}
fn current_uid() -> Option<u32> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("Uid:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}
fn expected_scope_path(unit: &str) -> Option<PathBuf> {
    let uid = current_uid()?;
    Some(PathBuf::from(format!(
        "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/app.slice/{unit}.scope"
    )))
}
pub fn unique_unit() -> String {
    format!(
        "harm-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
pub fn run_control_experiment(
    unit: &str,
    weight: u32,
    audit_path: &Path,
    workload_millis: u64,
) -> Result<(), String> {
    let scope = DelegatedScope::discover_and_delegate(unit)
        .map_err(|e| format!("delegation unavailable: {e:?}"))?;
    if let Some(parent) = audit_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::File::create(audit_path).map_err(|e| e.to_string())?;
    audit(
        audit_path,
        &audit_record(
            "scope_created",
            &scope.root,
            None,
            None,
            None,
            None,
            "created",
            "transient user scope with Delegate=yes",
        ),
    )
    .map_err(|e| e.to_string())?;
    let mut whitelist = PathWhitelist::default();
    whitelist.mark_created(scope.workload.clone());
    let mut controller = WeightController::new(
        FileCgroupIo,
        whitelist,
        ControlGuards::default(),
        audit_path.into(),
    );
    let action = ControlAction {
        workload: "cpu-burn".into(),
        cgroup: scope.workload.clone(),
        cpu_weight: weight,
        expires_at_seconds: f64::MAX,
        source: Source::Assumed,
    };
    let result = controller
        .apply(
            &action,
            ApplyOptions {
                dry_run: false,
                is_background: false,
                now_seconds: crate::monotonic_seconds(),
            },
        )
        .map_err(|e| format!("apply weight: {e:?}"))?;
    println!(
        "scope={} workload_cgroup={} original={} requested={} applied={} readback={}",
        scope.root.display(),
        scope.workload.display(),
        result.before,
        weight,
        result.after,
        result.readback
    );
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let plan = format!("{}:1.0", Duration::from_millis(workload_millis).as_nanos());
    let mut child = Command::new(exe)
        .arg("__burn")
        .arg("--phase-plan")
        .arg(plan)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Err(error) = scope.attach_pid(child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("workload attach failed: {error:?}"));
    }
    child
        .stdin
        .take()
        .ok_or("workload gate missing")?
        .write_all(b"g")
        .map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("CPU workload exited {status}"));
    }
    println!("workload_pid={} moved=true status={status}", child.id());
    controller
        .restore_all()
        .map_err(|e| format!("restore cpu.weight: {e:?}"))?;
    let restored = FileCgroupIo
        .read_weight(&scope.workload)
        .map_err(|e| e.to_string())?;
    scope
        .remove_workload()
        .map_err(|e| format!("cleanup workload cgroup failed: {e:?}"))?;
    println!("restored_weight={restored} workload_cgroup_removed=true");
    let text = fs::read_to_string(audit_path).map_err(|e| e.to_string())?;
    println!("audit_jsonl_begin\n{text}audit_jsonl_end");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Fake {
        value: Arc<Mutex<u32>>,
        mismatch: bool,
    }
    impl CgroupIo for Fake {
        fn read_weight(&self, _: &Path) -> io::Result<u32> {
            let value = *self.value.lock().unwrap();
            Ok(if self.mismatch && value != 100 {
                value + 1
            } else {
                value
            })
        }
        fn write_weight(&self, _: &Path, value: u32) -> io::Result<()> {
            *self.value.lock().unwrap() = value;
            Ok(())
        }
    }
    fn test_dir(label: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("experiments/data");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "harm-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }
    fn manager(backend: Fake, path: &Path, audit: &Path) -> WeightController<Fake> {
        let mut whitelist = PathWhitelist::default();
        whitelist.mark_created(path.into());
        WeightController::new(
            backend,
            whitelist,
            ControlGuards {
                max_delta: 500,
                min_background_weight: 100,
                max_frequency: Duration::ZERO,
            },
            audit.into(),
        )
    }
    fn action(path: &Path, weight: u32) -> ControlAction {
        ControlAction {
            workload: "test".into(),
            cgroup: path.into(),
            cpu_weight: weight,
            expires_at_seconds: 100.0,
            source: Source::Assumed,
        }
    }
    fn options(dry_run: bool, now_seconds: f64, is_background: bool) -> ApplyOptions {
        ApplyOptions {
            dry_run,
            is_background,
            now_seconds,
        }
    }
    #[test]
    fn untrusted_paths_refused_and_audited() {
        let dir = test_dir("control");
        let path = dir.join("not-allowed");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("cpu.weight"), "100").unwrap();
        let audit_path = dir.join("audit.jsonl");
        // Fake.write_weight ignores the target path and mutates this shared cell, so if
        // the controller had written anything the value would move off 100 no matter
        // which cgroup was named. Holding a handle is what turns "an error came back"
        // into proof that no write occurred: the refusal must be a real no-op.
        let value = Arc::new(Mutex::new(100));
        let backend = Fake {
            value: value.clone(),
            mismatch: false,
        };
        let mut ctl = manager(backend, &dir.join("allowed"), &audit_path);
        assert!(matches!(
            ctl.apply(&action(&path, 200), options(false, 1.0, false)),
            Err(ControlError::PathNotWhitelisted(_))
        ));
        assert_eq!(*value.lock().unwrap(), 100, "refused action must not write");
        assert!(fs::read_to_string(&audit_path).unwrap().contains("refused"));
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn dry_run_audits_without_write() {
        let dir = test_dir("dry");
        let file = dir.join("cpu.weight");
        fs::write(&file, "100").unwrap();
        let backend = Fake {
            value: Arc::new(Mutex::new(100)),
            mismatch: false,
        };
        let mut ctl = manager(backend, &dir, &dir.join("audit.jsonl"));
        let result = ctl
            .apply(&action(&dir, 200), options(true, 1.0, false))
            .unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "100");
        assert!(
            fs::read_to_string(dir.join("audit.jsonl"))
                .unwrap()
                .contains("dry_run")
        );
        assert_eq!(result.after, 100);
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn mismatch_rolls_back_and_audits() {
        let dir = test_dir("rollback");
        let value = Arc::new(Mutex::new(100));
        let backend = Fake {
            value: value.clone(),
            mismatch: true,
        };
        let mut ctl = manager(backend, &dir, &dir.join("audit.jsonl"));
        assert!(matches!(
            ctl.apply(&action(&dir, 200), options(false, 1.0, false)),
            Err(ControlError::ReadbackMismatch { .. })
        ));
        assert_eq!(*value.lock().unwrap(), 100);
        assert!(
            fs::read_to_string(dir.join("audit.jsonl"))
                .unwrap()
                .contains("rolled_back")
        );
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn weight_step_clamps_and_protects_background_minimum() {
        let dir = test_dir("weight");
        let value = Arc::new(Mutex::new(500));
        let backend = Fake {
            value: value.clone(),
            mismatch: false,
        };
        let mut ctl = manager(backend, &dir, &dir.join("audit.jsonl"));
        let result = ctl
            .apply(&action(&dir, 9000), options(false, 1.0, true))
            .unwrap();
        assert_eq!(result.after, 1000);
        assert!(matches!(
            ctl.apply(&action(&dir, 1), options(false, 1.0, true)),
            Err(ControlError::ProtectedMinimum)
        ));
        ctl.restore_all().unwrap();
        assert_eq!(*value.lock().unwrap(), 500);
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn restore_event_records_applied_value_as_before() {
        let dir = test_dir("restore-audit");
        let value = Arc::new(Mutex::new(100));
        let backend = Fake {
            value: value.clone(),
            mismatch: false,
        };
        let audit_path = dir.join("audit.jsonl");
        let mut ctl = manager(backend, &dir, &audit_path);
        // Apply 237 so the weight in effect just before restore is 237.
        ctl.apply(&action(&dir, 237), options(false, 1.0, false))
            .unwrap();
        assert_eq!(*value.lock().unwrap(), 237);
        ctl.restore_all().unwrap();
        let rows: Vec<serde_json::Value> = fs::read_to_string(&audit_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let restore = rows
            .iter()
            .find(|row| row["event"] == "restore")
            .expect("restore audit event");
        assert_eq!(
            restore["before"], 237,
            "restore must record the applied value it restored from"
        );
        assert_eq!(restore["after"], 100);
        assert_eq!(restore["readback"], 100);
        assert_eq!(restore["result"], "restored");
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn expiry_and_oracle_provenance_are_refused() {
        let dir = test_dir("reject");
        let backend = Fake {
            value: Arc::new(Mutex::new(100)),
            mismatch: false,
        };
        let mut ctl = manager(backend, &dir, &dir.join("audit.jsonl"));
        assert_eq!(
            ctl.apply(&action(&dir, 200), options(false, 101.0, false))
                .unwrap_err(),
            ControlError::Expired
        );
        let mut oracle = action(&dir, 200);
        oracle.source = Source::Oracle;
        assert_eq!(
            ctl.apply(&oracle, options(false, 1.0, false)).unwrap_err(),
            ControlError::OracleNotProductionSafe
        );
        let audit = fs::read_to_string(dir.join("audit.jsonl")).unwrap();
        assert!(audit.contains("action expired") && audit.contains("offline-only"));
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn controller_restores_during_unwind() {
        let dir = test_dir("controller-unwind");
        let value = Arc::new(Mutex::new(100));
        let backend = Fake {
            value: value.clone(),
            mismatch: false,
        };
        let audit = dir.join("audit.jsonl");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut ctl = manager(backend, &dir, &audit);
            ctl.apply(&action(&dir, 200), options(false, 1.0, false))
                .unwrap();
            panic!("exercise controller restoration during unwind");
        }));
        assert!(result.is_err());
        assert_eq!(*value.lock().unwrap(), 100);
        assert!(
            fs::read_to_string(&audit)
                .unwrap()
                .contains("WeightController Drop restoration")
        );
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn controller_skeletons_share_observation_action_contract() {
        let dir = test_dir("controllers");
        let path = dir.join("workload");
        let observation = ControlObservation {
            workload: "background".into(),
            psi_cpu_some_avg10: Some(0.3),
            current_weight: 100,
            monotonic_seconds: 1.0,
            source: Source::Observed,
        };
        assert!(NativeController.decide(&observation, &path).is_none());
        let static_action = StaticController { weight: 300 }
            .decide(&observation, &path)
            .unwrap();
        assert_eq!(static_action.cpu_weight, 300);
        let reactive_action = ReactiveController {
            low_pressure_weight: 100,
            high_pressure_weight: 500,
            threshold: 0.2,
        }
        .decide(&observation, &path)
        .unwrap();
        assert_eq!(reactive_action.cpu_weight, 500);
        assert_eq!(reactive_action.source, Source::Observed);
        let trace = dir.join("future.jsonl");
        fs::write(&trace, "{\"target\":\"cpu.pressure\",\"observed_at_seconds\":2.0,\"value\":0.5,\"source\":\"observed\"}\n").unwrap();
        let mut oracle = OracleController::from_trace_file(&trace, "cpu.pressure".into()).unwrap();
        let oracle_action = oracle.decide(&observation, &path).unwrap();
        assert_eq!(oracle_action.source, Source::Oracle);
        assert_eq!(oracle_action.cpu_weight, 1000);
        let _ = fs::remove_dir_all(dir);
    }

    // --- FuturePhaseController: the study Oracle. Must use only the trace + present clock. ---

    fn timeline_trace(dir: &Path) -> PathBuf {
        // idle [0,0.8), burst [0.8,2.0), steady [2.0,3.0), spike [3.0,4.5). Protect burst+spike.
        let trace = dir.join("future-timeline.jsonl");
        fs::write(
            &trace,
            concat!(
                "{\"kind\":\"phase\",\"phase\":\"idle\",\"monotonic_seconds\":0.0,\"duration_seconds\":0.8,\"duty_cycle\":0.05}\n",
                "{\"kind\":\"phase\",\"phase\":\"burst\",\"monotonic_seconds\":0.8,\"duration_seconds\":1.2,\"duty_cycle\":0.9}\n",
                "{\"kind\":\"phase\",\"phase\":\"steady\",\"monotonic_seconds\":2.0,\"duration_seconds\":1.0,\"duty_cycle\":0.5}\n",
                "{\"kind\":\"phase\",\"phase\":\"spike\",\"monotonic_seconds\":3.0,\"duration_seconds\":1.5,\"duty_cycle\":0.98}\n",
            ),
        )
        .unwrap();
        trace
    }

    fn study_oracle(dir: &Path) -> FuturePhaseController {
        FuturePhaseController::from_trace_file(
            &timeline_trace(dir),
            1.0, // horizon: look one second ahead
            vec!["burst".into(), "spike".into()],
            20,   // protected weight (foreground-protective)
            1000, // free weight
        )
        .unwrap()
    }

    #[test]
    fn future_oracle_reacts_ahead_of_transition() {
        let dir = test_dir("future-oracle-ahead");
        let ctl = study_oracle(&dir);
        // At t=0.5 the *current* phase is idle, but one second ahead (t=1.5) is burst -> protect now.
        assert_eq!(
            ctl.desired_weight(0.5),
            20,
            "must lower weight before burst begins at 0.8"
        );
        // Deep inside idle with no protected phase in horizon -> free weight.
        assert_eq!(ctl.desired_weight(-0.4), 1000); // t+horizon=0.6 still idle
        // Inside the final spike (t=3.5) it keeps protecting even though +1s is past the trace end,
        // because the *current* recorded phase is protected — no extrapolation, just present knowledge.
        assert_eq!(
            ctl.desired_weight(3.5),
            20,
            "must protect through the final recorded phase"
        );
        assert_eq!(ctl.desired_weight(4.4), 20); // still inside spike [3.0,4.5)
        // Past the whole timeline there is no known future -> free weight (never invents a phase).
        assert_eq!(ctl.desired_weight(4.6), 1000);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn future_oracle_decision_ignores_live_state() {
        // Leakage proof: the decision is invariant to every live field (psi, current weight, source)
        // and still produced when no live metric exists at all (psi = None). Only monotonic time matters.
        let dir = test_dir("future-oracle-no-live");
        let path = dir.join("workload");
        let mut ctl = study_oracle(&dir);
        let base = ControlObservation {
            workload: "bg".into(),
            psi_cpu_some_avg10: None, // no live pressure available at all
            current_weight: 7,        // garbage
            monotonic_seconds: 0.5,
            source: Source::Observed,
        };
        let a = ctl.decide(&base, &path).expect("decides with psi=None");
        assert_eq!(a.cpu_weight, 20);
        for (psi, w) in [(Some(0.0), 1u32), (Some(99.0), 10_000u32)] {
            let mut o = base.clone();
            o.psi_cpu_some_avg10 = psi;
            o.current_weight = w;
            o.source = Source::Predicted;
            assert_eq!(ctl.decide(&o, &path).unwrap().cpu_weight, a.cpu_weight);
        }
        let _ = fs::remove_dir_all(dir);
    }
}
