//! `coro-bench`: one benchmark run per invocation (JSON to `--out`), or
//! `--sanity` (mutual-exclusion check over every lock).
//!
//! ```text
//! coro-bench --lock ces --workers 8 --clients 64 --heavy-ratio 8 \
//!            --duration-ms 2000 --out results/ces-w8-h8-r1.json
//! coro-bench --lock ces --ces-chain-bound 64 --wake-placement home ...
//! coro-bench --lock fcpq --pass-limit 8 --rotate-combiner --wake-placement home ...
//! coro-bench --lock dispatch-pq --starvation-clamp 8 --wake-placement home ...
//! coro-bench --lock cfl --cfl-prewake off --starvation-clamp 64 ...
//! coro-bench --lock co-pq --co-step-aside remote --ces-chain-bound 64 --wake-placement home ...
//! coro-bench --sanity
//! ```
//!
//! Variant label written to the JSON (`lock` field): base id plus one suffix
//! per non-default knob, e.g. `ces-k64-home`, `fcpq-h8-rotate`, `fc-remote`,
//! `dispatch-home`, `fc-noyield`, `dispatch-pq-home-c8`, `cfl-nopre-c64`.

use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use coro_delegation::executor::Placement;
use coro_delegation::lock::DelegationLock;
use coro_delegation::locks::actor::{Actor, ActorOptions};
use coro_delegation::locks::ces::{Ces, CesOptions};
use coro_delegation::locks::cfl::{self, Cfl, CflOptions, CflStats, Scan, Shuffle};
use coro_delegation::locks::co_mutex::{
    CoFifo, CoOptions, CoPq, StepAside, DEFAULT_CHAIN_BOUND, DEFAULT_STARVATION_CLAMP,
};
use coro_delegation::locks::dispatch::Dispatch;
use coro_delegation::locks::dispatch_pq::{self, DispatchPq, DispatchPqOptions, HandoffStats};
use coro_delegation::locks::fc::{Fc, FcOptions, WakePlacement};
use coro_delegation::locks::fc_pq::{self, FcPq, FcPqOptions, NewcomerInit, WaitStats};
use coro_delegation::workload::{self, Config, ParallelMode, Report, Shared};

const LOCKS: &[&str] = &[
    "dispatch",
    "ces",
    "fc",
    "fcpq",
    "actor",
    "actor-inline",
    "dispatch-pq",
    "cfl",
    "co-fifo",
    "co-pq",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum WakePlacementArg {
    Default,
    Remote,
    Home,
}

impl WakePlacementArg {
    fn executor(self) -> Placement {
        match self {
            WakePlacementArg::Default => Placement::Default,
            WakePlacementArg::Remote => Placement::Remote,
            WakePlacementArg::Home => Placement::Home,
        }
    }

    fn fc(self) -> WakePlacement {
        match self {
            WakePlacementArg::Default => WakePlacement::Default,
            WakePlacementArg::Remote => WakePlacement::Remote,
            WakePlacementArg::Home => WakePlacement::Home,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            WakePlacementArg::Default => "",
            WakePlacementArg::Remote => "-remote",
            WakePlacementArg::Home => "-home",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum NewcomerInitArg {
    Mean,
    Zero,
    Min,
    Median,
}

impl NewcomerInitArg {
    fn fc_pq(self) -> NewcomerInit {
        match self {
            NewcomerInitArg::Mean => NewcomerInit::Mean,
            NewcomerInitArg::Zero => NewcomerInit::Zero,
            NewcomerInitArg::Min => NewcomerInit::Min,
            NewcomerInitArg::Median => NewcomerInit::Median,
        }
    }

    fn name(self) -> &'static str {
        match self {
            NewcomerInitArg::Mean => "mean",
            NewcomerInitArg::Zero => "zero",
            NewcomerInitArg::Min => "min",
            NewcomerInitArg::Median => "median",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CflShuffleArg {
    Usage,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OnOffArg {
    On,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CflScanArg {
    Full,
    Overlap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ParallelModeArg {
    Spin,
    Yield,
}

impl ParallelModeArg {
    fn mode(self) -> ParallelMode {
        match self {
            ParallelModeArg::Spin => ParallelMode::Spin,
            ParallelModeArg::Yield => ParallelMode::Yield,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum StepAsideArg {
    Remote,
    Home,
    None,
}

impl StepAsideArg {
    fn co(self) -> StepAside {
        match self {
            StepAsideArg::Remote => StepAside::Remote,
            StepAsideArg::Home => StepAside::Home,
            StepAsideArg::None => StepAside::None,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "coro-bench",
    about = "Delegation locks on a coroutine executor"
)]
struct Cli {
    /// Lock variant: dispatch | ces | fc | fcpq | actor | actor-inline | dispatch-pq | cfl |
    /// co-fifo | co-pq
    #[arg(long, default_value = "dispatch")]
    lock: String,
    /// Executor workers (each pinned to its own physical core)
    #[arg(long, default_value_t = 8)]
    workers: usize,
    /// Client tasks (half light, half heavy)
    #[arg(long, default_value_t = 64)]
    clients: usize,
    /// Bystander tasks (default: = workers)
    #[arg(long)]
    bystanders: Option<usize>,
    /// Heavy critical-section cost as a multiple of light
    #[arg(long, default_value_t = 8)]
    heavy_ratio: u64,
    /// Light critical-section spin, TSC cycles (plus one BTreeMap insert)
    #[arg(long, default_value_t = 1000)]
    light_cs_cycles: u64,
    /// Parallel work between requests, TSC cycles (default: 4 x light CS)
    #[arg(long)]
    parallel_work_cycles: Option<u64>,
    /// Bystander work per poll, TSC cycles (default: = light CS)
    #[arg(long)]
    bystander_work_cycles: Option<u64>,
    /// Random key space of the BTreeMap inserts
    #[arg(long, default_value_t = 65_536)]
    key_space: u64,
    /// Measurement window after warm-up, milliseconds
    #[arg(long, default_value_t = 2000)]
    duration_ms: u64,
    /// Warm-up, milliseconds (not recorded)
    #[arg(long, default_value_t = 200)]
    warmup_ms: u64,
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Executor balancing-steal interval in polls (0 = steal only when idle)
    #[arg(long, default_value_t = coro_delegation::executor::DEFAULT_BALANCE_INTERVAL)]
    balance_interval: u32,
    /// Client parallel work after each op, all locks: spin (same poll,
    /// original) | yield (`yield_now().await`, then spin). Recorded as
    /// `config.parallel_mode`, not in the label
    #[arg(long, value_enum, default_value_t = ParallelModeArg::Spin)]
    parallel_mode: ParallelModeArg,
    /// Output JSON path (stdout if omitted)
    #[arg(long)]
    out: Option<PathBuf>,
    /// Mutual-exclusion check: every lock, 8 workers, 500 ms
    #[arg(long, default_value_t = false)]
    sanity: bool,

    // --- placement (all locks) -------------------------------------------
    /// Placement of lock-issued wakes: dispatch / dispatch-pq handoff, CES
    /// and co-fifo / co-pq chain-break handoff, every fc/fcpq wake (served
    /// waiters, next combiner, self-yield), actor client completions
    /// (`default` keeps the variant's own: actor default, actor-inline
    /// remote)
    #[arg(long, value_enum, default_value_t = WakePlacementArg::Default)]
    wake_placement: WakePlacementArg,

    // --- ces -------------------------------------------------------------
    /// ces: break the inline chain after this many handoffs (label -k<K>).
    /// co-fifo / co-pq: the same, default 64, 0 = unbounded (label always
    /// -k<K>)
    #[arg(long)]
    ces_chain_bound: Option<u32>,
    /// ces: break the inline chain after this many cycles (label -t<T>)
    #[arg(long)]
    ces_chain_budget_cycles: Option<u64>,

    // --- fc / fcpq -------------------------------------------------------
    /// fc/fcpq/actor: maximum closures per combining pass, H (label -h<H>)
    #[arg(long)]
    pass_limit: Option<usize>,
    /// fcpq: bound a combining pass to this many cycles (label -t<T>)
    #[arg(long)]
    pass_budget_cycles: Option<u64>,
    /// fcpq: rotate the combiner role at every pass end
    #[arg(long, default_value_t = false)]
    rotate_combiner: bool,
    /// fcpq: credit combining cycles against the combiner's usage
    #[arg(long, default_value_t = false)]
    credit_combining: bool,
    /// fcpq: elect the highest-usage waiter as next combiner
    #[arg(long, default_value_t = false)]
    elect_max_usage: bool,
    /// fc/fcpq: disable the combiner's cooperative yield after a pass
    /// (original flat-combining behaviour; label -noyield)
    #[arg(long, default_value_t = false)]
    no_combiner_yield: bool,
    /// fcpq: passes a queued request may wait before its key is clamped to
    /// the heap minimum, 0 = never (default 8; label -c<N>). dispatch-pq:
    /// the same in handoffs (default 16). cfl: handoffs a waiter may watch
    /// go to others before it gets headship regardless of usage (default
    /// 256). co-pq: in handoffs, default 256 (label always -c<N>)
    #[arg(long)]
    starvation_clamp: Option<u64>,
    /// fcpq / dispatch-pq: admission usage of a client's first queued
    /// request: mean (running mean request cost, default), zero, or min /
    /// median of the queued entries' usage (label -n<init>)
    #[arg(long, value_enum)]
    newcomer_init: Option<NewcomerInitArg>,
    /// fcpq / dispatch-pq: count queue waits in combining passes (fcpq) or
    /// handoffs (dispatch-pq), written as `fcpq_wait` in the JSON
    /// (instrumentation, no label suffix)
    #[arg(long, default_value_t = false)]
    fcpq_wait_stats: bool,

    // --- dispatch-pq / cfl -----------------------------------------------
    /// dispatch-pq / cfl: record acquisition paths and the handoff cycle
    /// breakdown, written as `handoff` (dispatch-pq) / `cfl` (cfl) in the
    /// JSON (instrumentation, no label suffix)
    #[arg(long, default_value_t = false)]
    handoff_stats: bool,

    // --- cfl ---------------------------------------------------------------
    /// cfl: successor choice by the queue head: usage (minimum charged
    /// cycles among the waiters behind it, default) or off (MCS FIFO; label
    /// -noshfl)
    #[arg(long, value_enum, default_value_t = CflShuffleArg::Usage)]
    cfl_shuffle: CflShuffleArg,
    /// cfl: wake a node when it becomes queue head (on, default) or only
    /// after the owner's release (off; label -nopre)
    #[arg(long, value_enum, default_value_t = OnOffArg::On)]
    cfl_prewake: OnOffArg,
    /// cfl: head scan policy: full (finish the scan before each acquisition
    /// attempt, default) or overlap (scan only while the lock is held, CFL
    /// rule; label -ovl)
    #[arg(long, value_enum, default_value_t = CflScanArg::Full)]
    cfl_scan: CflScanArg,
    /// cfl: head spin budget per poll before parking, TSC cycles (default
    /// 8000; label -spin<N>)
    #[arg(long)]
    head_spin_cycles: Option<u64>,

    // --- co-fifo / co-pq -------------------------------------------------
    /// co-fifo / co-pq: where `unlock().await` puts the releaser after
    /// waking the grantee inline: remote (injector), home (this worker's
    /// inbox), none (no suspension, as a synchronous drop); label -s<mode>
    #[arg(long, value_enum, default_value_t = StepAsideArg::Remote)]
    co_step_aside: StepAsideArg,
}

impl Cli {
    fn config(&self) -> Config {
        Config {
            workers: self.workers,
            clients: self.clients,
            bystanders: self.bystanders.unwrap_or(self.workers),
            heavy_ratio: self.heavy_ratio,
            light_cs_cycles: self.light_cs_cycles,
            parallel_work_cycles: self
                .parallel_work_cycles
                .unwrap_or(4 * self.light_cs_cycles),
            bystander_work_cycles: self.bystander_work_cycles.unwrap_or(self.light_cs_cycles),
            key_space: self.key_space.max(1),
            duration_ms: self.duration_ms,
            warmup_ms: self.warmup_ms,
            seed: self.seed,
            unique_keys: false,
            balance_interval: self.balance_interval,
            parallel_mode: self.parallel_mode.mode(),
        }
    }

    fn ces_options(&self) -> CesOptions {
        CesOptions {
            chain_bound: self.ces_chain_bound,
            chain_budget_cycles: self.ces_chain_budget_cycles,
            break_placement: self.wake_placement.executor(),
        }
    }

    fn ces_label(&self) -> String {
        let mut label = String::from("ces");
        if let Some(k) = self.ces_chain_bound {
            label.push_str(&format!("-k{k}"));
        }
        if let Some(t) = self.ces_chain_budget_cycles {
            label.push_str(&format!("-t{t}"));
        }
        label.push_str(self.wake_placement.suffix());
        label
    }

    fn fc_options(&self) -> FcOptions {
        let defaults = FcOptions::default();
        FcOptions {
            yield_after_combine: !self.no_combiner_yield,
            wake_placement: self.wake_placement.fc(),
            pass_limit: self.pass_limit.unwrap_or(defaults.pass_limit),
        }
    }

    fn fcpq_options(&self) -> FcPqOptions {
        let core = self.fc_options();
        let defaults = FcPqOptions::default();
        FcPqOptions {
            pass_budget_cycles: self.pass_budget_cycles,
            rotate_combiner: self.rotate_combiner,
            credit_combining: self.credit_combining,
            elect_max_usage: self.elect_max_usage,
            yield_after_combine: core.yield_after_combine,
            wake_placement: core.wake_placement,
            pass_limit: core.pass_limit,
            starvation_clamp: self.starvation_clamp.unwrap_or(defaults.starvation_clamp),
            newcomer_init: self
                .newcomer_init
                .map_or(defaults.newcomer_init, NewcomerInitArg::fc_pq),
            record_waits: self.fcpq_wait_stats,
        }
    }

    /// Suffixes shared by fc and fcpq.
    fn fc_suffixes(&self, label: &mut String) {
        if let Some(h) = self.pass_limit {
            label.push_str(&format!("-h{h}"));
        }
        if self.no_combiner_yield {
            label.push_str("-noyield");
        }
    }

    fn fc_label(&self) -> String {
        let mut label = String::from("fc");
        self.fc_suffixes(&mut label);
        label.push_str(self.wake_placement.suffix());
        label
    }

    fn fcpq_label(&self) -> String {
        let mut label = String::from("fcpq");
        self.fc_suffixes(&mut label);
        if let Some(t) = self.pass_budget_cycles {
            label.push_str(&format!("-t{t}"));
        }
        if self.rotate_combiner {
            label.push_str("-rotate");
        }
        if self.credit_combining {
            label.push_str("-credit");
        }
        if self.elect_max_usage {
            label.push_str("-elect");
        }
        label.push_str(self.wake_placement.suffix());
        if let Some(c) = self.starvation_clamp {
            label.push_str(&format!("-c{c}"));
        }
        if let Some(n) = self.newcomer_init {
            label.push_str(&format!("-n{}", n.name()));
        }
        label
    }

    fn dispatch_label(&self) -> String {
        format!("dispatch{}", self.wake_placement.suffix())
    }

    fn dispatch_pq_options(&self) -> DispatchPqOptions {
        let defaults = DispatchPqOptions::default();
        DispatchPqOptions {
            wake_placement: self.wake_placement.executor(),
            queue: FcPqOptions {
                starvation_clamp: self
                    .starvation_clamp
                    .unwrap_or(defaults.queue.starvation_clamp),
                newcomer_init: self
                    .newcomer_init
                    .map_or(defaults.queue.newcomer_init, NewcomerInitArg::fc_pq),
                record_waits: self.fcpq_wait_stats,
                ..defaults.queue
            },
            record_handoffs: self.handoff_stats,
        }
    }

    /// Same suffix order as fcpq: placement, then clamp, then newcomer init.
    fn dispatch_pq_label(&self) -> String {
        let mut label = String::from("dispatch-pq");
        label.push_str(self.wake_placement.suffix());
        if let Some(c) = self.starvation_clamp {
            label.push_str(&format!("-c{c}"));
        }
        if let Some(n) = self.newcomer_init {
            label.push_str(&format!("-n{}", n.name()));
        }
        label
    }

    /// cfl's own wake placement is `home`; `--wake-placement default` keeps
    /// it (as for actor).
    fn cfl_options(&self) -> CflOptions {
        let d = CflOptions::default();
        CflOptions {
            shuffle: match self.cfl_shuffle {
                CflShuffleArg::Usage => Shuffle::Usage,
                CflShuffleArg::Off => Shuffle::Off,
            },
            scan: match self.cfl_scan {
                CflScanArg::Full => Scan::Full,
                CflScanArg::Overlap => Scan::Overlap,
            },
            prewake: self.cfl_prewake == OnOffArg::On,
            starvation_clamp: self.starvation_clamp.unwrap_or(d.starvation_clamp),
            head_spin_cycles: self.head_spin_cycles.unwrap_or(d.head_spin_cycles),
            wake_placement: match self.wake_placement {
                WakePlacementArg::Default => d.wake_placement,
                p => p.fc(),
            },
            record_handoffs: self.handoff_stats,
        }
    }

    /// `cfl[-noshfl][-nopre][-ovl][-remote][-c<N>][-spin<N>]`.
    fn cfl_label(&self) -> String {
        let mut label = String::from("cfl");
        if self.cfl_shuffle == CflShuffleArg::Off {
            label.push_str("-noshfl");
        }
        if self.cfl_prewake == OnOffArg::Off {
            label.push_str("-nopre");
        }
        if self.cfl_scan == CflScanArg::Overlap {
            label.push_str("-ovl");
        }
        if self.cfl_options().wake_placement != CflOptions::default().wake_placement {
            label.push_str(self.wake_placement.suffix());
        }
        if let Some(c) = self.starvation_clamp {
            label.push_str(&format!("-c{c}"));
        }
        if let Some(s) = self.head_spin_cycles {
            label.push_str(&format!("-spin{s}"));
        }
        label
    }

    /// `variant` (`ActorOptions::plain` / `inline`) with the CLI's
    /// non-default knobs applied.
    fn actor_options(&self, variant: ActorOptions) -> ActorOptions {
        ActorOptions {
            wake_placement: match self.wake_placement {
                WakePlacementArg::Default => variant.wake_placement,
                p => p.fc(),
            },
            pass_limit: self.pass_limit.unwrap_or(variant.pass_limit),
            ..variant
        }
    }

    fn actor_label(&self, base: &str, variant: ActorOptions) -> String {
        let mut label = String::from(base);
        if let Some(h) = self.pass_limit {
            label.push_str(&format!("-h{h}"));
        }
        if self.actor_options(variant).wake_placement != variant.wake_placement {
            label.push_str(self.wake_placement.suffix());
        }
        label
    }

    fn co_options(&self) -> CoOptions {
        let d = CoOptions::default();
        CoOptions {
            step_aside: self.co_step_aside.co(),
            chain_bound: match self.ces_chain_bound {
                None => Some(DEFAULT_CHAIN_BOUND),
                Some(0) => None,
                k => k,
            },
            break_placement: self.wake_placement.executor(),
            queue: FcPqOptions {
                starvation_clamp: self.starvation_clamp.unwrap_or(DEFAULT_STARVATION_CLAMP),
                newcomer_init: self
                    .newcomer_init
                    .map_or(d.queue.newcomer_init, NewcomerInitArg::fc_pq),
                record_waits: self.fcpq_wait_stats,
                ..d.queue
            },
        }
    }

    /// `co-fifo-s<step>-k<K>[-home|-remote]`, `co-pq-...-c<N>[-n<init>]`.
    fn co_label(&self, base: &str) -> String {
        let o = self.co_options();
        let mut label = format!(
            "{base}-s{}-k{}{}",
            o.step_aside.label(),
            o.chain_bound.unwrap_or(0),
            self.wake_placement.suffix()
        );
        if base == "co-pq" {
            label.push_str(&format!("-c{}", o.queue.starvation_clamp));
            if let Some(n) = self.newcomer_init {
                label.push_str(&format!("-n{}", n.name()));
            }
        }
        label
    }
}

fn run_lock(cli: &Cli, cfg: &Config, tsc_hz: f64) -> Report {
    match cli.lock.as_str() {
        "dispatch" => {
            let placement = cli.wake_placement.executor();
            workload::run_benchmark(cfg, tsc_hz, &cli.dispatch_label(), move |data| {
                Dispatch::with_placement(data, placement)
            })
        }
        "dispatch-pq" => {
            let opts = cli.dispatch_pq_options();
            workload::run_benchmark(cfg, tsc_hz, &cli.dispatch_pq_label(), move |data| {
                DispatchPq::with_options(data, opts)
            })
        }
        "cfl" => {
            let opts = cli.cfl_options();
            workload::run_benchmark(cfg, tsc_hz, &cli.cfl_label(), move |data| {
                Cfl::with_options(data, opts)
            })
        }
        "ces" => {
            let opts = cli.ces_options();
            workload::run_benchmark(cfg, tsc_hz, &cli.ces_label(), move |data| {
                Ces::with_options(data, opts)
            })
        }
        "fc" => {
            let opts = cli.fc_options();
            workload::run_benchmark(cfg, tsc_hz, &cli.fc_label(), move |data| {
                Fc::with_options(data, opts)
            })
        }
        "fcpq" => {
            let opts = cli.fcpq_options();
            workload::run_benchmark(cfg, tsc_hz, &cli.fcpq_label(), move |data| {
                FcPq::with_options(data, opts)
            })
        }
        base @ ("actor" | "actor-inline") => {
            let variant = if base == "actor" {
                ActorOptions::plain()
            } else {
                ActorOptions::inline()
            };
            let opts = cli.actor_options(variant);
            workload::run_benchmark(cfg, tsc_hz, &cli.actor_label(base, variant), move |data| {
                Actor::with_options(data, opts)
            })
        }
        "co-fifo" => {
            let opts = cli.co_options();
            workload::run_co_benchmark(cfg, tsc_hz, &cli.co_label("co-fifo"), move |data| {
                CoFifo::with_options(data, opts)
            })
        }
        "co-pq" => {
            let opts = cli.co_options();
            workload::run_co_benchmark(cfg, tsc_hz, &cli.co_label("co-pq"), move |data| {
                CoPq::with_options(data, opts)
            })
        }
        other => {
            eprintln!("unknown lock {other:?}; known: {}", LOCKS.join(", "));
            std::process::exit(2);
        }
    }
}

fn sanity_lock(id: &str, cfg: &Config) -> Result<workload::SanityOutcome, String> {
    match id {
        "dispatch" => workload::sanity(cfg, Dispatch::<Shared>::new),
        "ces" => workload::sanity(cfg, Ces::<Shared>::new),
        "fc" => workload::sanity(cfg, Fc::<Shared>::new),
        "fcpq" => workload::sanity(cfg, FcPq::<Shared>::new),
        "actor" => workload::sanity(cfg, Actor::<Shared>::new),
        "actor-inline" => workload::sanity(cfg, |data| {
            Actor::with_options(data, ActorOptions::inline())
        })
        .map(|o| workload::SanityOutcome {
            lock: "actor-inline",
            ..o
        }),
        "dispatch-pq" => workload::sanity(cfg, DispatchPq::<Shared>::new),
        "cfl" => workload::sanity(cfg, Cfl::<Shared>::new),
        "co-fifo" => {
            workload::sanity_co(cfg, |data| CoFifo::with_options(data, CoOptions::default()))
        }
        "co-pq" => workload::sanity_co(cfg, |data| CoPq::with_options(data, CoOptions::default())),
        other => Err(format!("{other}: not available")),
    }
}

fn main() {
    let cli = Cli::parse();
    let tsc_hz = workload::estimate_tsc_hz();

    if cli.sanity {
        let cfg = Config {
            workers: 8,
            duration_ms: 500,
            warmup_ms: 0,
            unique_keys: true,
            ..cli.config()
        };
        let mut failed = false;
        for id in LOCKS {
            match sanity_lock(id, &cfg) {
                Ok(o) => eprintln!(
                    "sanity {:<9} PASS  ops={} len={}",
                    o.lock, o.total_ops, o.final_len
                ),
                Err(e) => {
                    failed = true;
                    eprintln!("sanity {id:<9} FAIL  {e}");
                }
            }
        }
        if failed {
            std::process::exit(1);
        }
        return;
    }

    let cfg = cli.config();
    let report = run_lock(&cli, &cfg, tsc_hz);
    print_summary(&report);
    let fcpq_wait = if cli.lock == "fcpq" || cli.lock == "dispatch-pq" || cli.lock == "co-pq" {
        fc_pq::take_wait_stats()
    } else {
        None
    };
    if let Some(w) = &fcpq_wait {
        eprintln!(
            "  {} wait ({}): served={} passes={} promoted={} max={}",
            cli.lock,
            if cli.lock == "fcpq" {
                "passes"
            } else {
                "handoffs"
            },
            w.served,
            w.passes,
            w.promoted,
            w.max_wait
        );
    }
    let handoff = if cli.lock == "dispatch-pq" {
        dispatch_pq::take_handoff_stats()
    } else {
        None
    };
    if let Some(h) = &handoff {
        let per = |c: u64| c as f64 / h.handoffs.max(1) as f64;
        eprintln!(
            "  handoff: fast={} free={} handoffs={} per handoff: spin={:.0} queue={:.0} grant->start={:.0} cycles",
            h.fast,
            h.free,
            h.handoffs,
            per(h.release_spin_cycles),
            per(h.release_queue_cycles),
            per(h.grant_to_start_cycles)
        );
    }
    let cfl_stats = if cli.lock == "cfl" {
        cfl::take_handoff_stats()
    } else {
        None
    };
    if let Some(s) = &cfl_stats {
        let per = |c: u64| c as f64 / s.handoffs.max(1) as f64;
        eprintln!(
            "  cfl: fast={} handoffs={} per handoff: release={:.0} react={:.0} pass={:.0} cycles; spin/late/woken={}/{}/{} scan={:.0} (on path {:.0}) visited={:.1} parks={} max_wait={}",
            s.fast,
            s.handoffs,
            per(s.release_cycles),
            per(s.react_cycles),
            per(s.pass_cycles),
            s.acq_spin,
            s.acq_late,
            s.acq_woken,
            per(s.scan_cycles),
            per(s.scan_on_path_cycles),
            per(s.scan_visited),
            s.parks,
            s.max_wait
        );
    }
    let out = Output {
        report: &report,
        fcpq_wait: fcpq_wait.map(wait_json),
        handoff: handoff.map(handoff_json),
        cfl: cfl_stats.map(cfl_json),
    };
    let json = serde_json::to_string_pretty(&out).expect("serialize report");
    match &cli.out {
        Some(path) => {
            if let Some(dir) = path.parent() {
                if !dir.as_os_str().is_empty() {
                    std::fs::create_dir_all(dir).expect("create output directory");
                }
            }
            std::fs::write(path, json).expect("write report");
            eprintln!("wrote {}", path.display());
        }
        None => println!("{json}"),
    }
}

/// The report plus lock-specific extras in one JSON object: the `Report`
/// fields in their usual order, then the extras.
#[derive(serde::Serialize)]
struct Output<'a> {
    #[serde(flatten)]
    report: &'a Report,
    /// fcpq / dispatch-pq: queue waits in combining passes / handoffs
    /// (`fc_pq::WaitStats`; `passes` counts ticks; `wait_hist[i]` = requests
    /// served after exactly i ticks, the last bucket open-ended).
    #[serde(skip_serializing_if = "Option::is_none")]
    fcpq_wait: Option<serde_json::Value>,
    /// dispatch-pq only: acquisition paths and handoff cycle sums
    /// (`dispatch_pq::HandoffStats`).
    #[serde(skip_serializing_if = "Option::is_none")]
    handoff: Option<serde_json::Value>,
    /// cfl only: acquisition paths, handoff cycle breakdown, scan / spin /
    /// park counters and waits in handoffs (`cfl::CflStats`).
    #[serde(skip_serializing_if = "Option::is_none")]
    cfl: Option<serde_json::Value>,
}

fn wait_json(w: WaitStats) -> serde_json::Value {
    serde_json::json!({
        "passes": w.passes,
        "served": w.served,
        "promoted": w.promoted,
        "max_wait": w.max_wait,
        "wait_hist": w.wait_hist.to_vec(),
    })
}

fn handoff_json(h: HandoffStats) -> serde_json::Value {
    serde_json::json!({
        "fast": h.fast,
        "free": h.free,
        "handoffs": h.handoffs,
        "release_spin_cycles": h.release_spin_cycles,
        "release_queue_cycles": h.release_queue_cycles,
        "grant_to_start_cycles": h.grant_to_start_cycles,
        "enqueues": h.enqueues,
        "enqueue_cycles": h.enqueue_cycles,
    })
}

fn cfl_json(s: CflStats) -> serde_json::Value {
    serde_json::json!({
        "fast": s.fast,
        "handoffs": s.handoffs,
        "release_cycles": s.release_cycles,
        "react_cycles": s.react_cycles,
        "pass_cycles": s.pass_cycles,
        "acq_spin": s.acq_spin,
        "acq_spin_react": s.acq_spin_react,
        "acq_late": s.acq_late,
        "acq_late_react": s.acq_late_react,
        "acq_woken": s.acq_woken,
        "acq_woken_react": s.acq_woken_react,
        "grants": s.grants,
        "grant_to_poll_cycles": s.grant_to_poll_cycles,
        "early_heads": s.early_heads,
        "scan_cycles": s.scan_cycles,
        "scan_on_path_cycles": s.scan_on_path_cycles,
        "scan_visited": s.scan_visited,
        "moves": s.moves,
        "promoted": s.promoted,
        "spin_cycles": s.spin_cycles,
        "parks": s.parks,
        "park_rechecks": s.park_rechecks,
        "head_wakes": s.head_wakes,
        "max_wait": s.max_wait,
        "wait_hist": s.wait_hist.to_vec(),
    })
}

fn print_summary(r: &Report) {
    let us = |c: u64| c as f64 / r.tsc_hz * 1e6;
    eprintln!(
        "{} workers={} clients={} heavy_ratio={} measured={:.3}s ops={} throughput={:.0} ops/s",
        r.lock,
        r.config.workers,
        r.config.clients,
        r.config.heavy_ratio,
        r.measured_secs,
        r.total_ops,
        r.throughput_ops_per_s
    );
    eprintln!(
        "  service_jain={} burden_jain={} total_combining={} cycles starved_clients={} starved_bystanders={} combiner_yields={}",
        fmt_opt(r.service_jain),
        fmt_opt(r.burden_jain),
        r.total_combining_cycles,
        r.starved_clients,
        r.starved_bystanders,
        r.total_combiner_yields
    );
    eprintln!(
        "  parallel_mode={} o={} cycles/op burden_foreign_jain={} total_foreign_cs={} cycles",
        r.config.parallel_mode.label(),
        r.o_cycles_per_op
            .map_or("null".to_string(), |o| format!("{o:.0}")),
        fmt_opt(r.burden_foreign_jain),
        r.total_foreign_cs_cycles
    );
    if let Some(s) = &r.co_mutex {
        eprintln!(
            "  co: fast={} free={} handoff={} step_asides={} chain_breaks={} sync_drops={}",
            s.fast, s.free, s.handoff, s.step_asides, s.chain_breaks, s.sync_drops
        );
    }
    eprintln!(
        "  placements default={} inline={} remote={} home={}  chains n={} p50={} max={}",
        r.placements.default,
        r.placements.inline,
        r.placements.remote,
        r.placements.home,
        r.chain_lengths.count,
        r.chain_lengths.p50,
        r.chain_lengths.max
    );
    for (name, c) in &r.classes {
        eprintln!(
            "  class {:<5} clients={} ops={} run p50={:.1}us p99={:.1}us",
            name,
            c.clients,
            c.ops,
            us(c.run_latency.p50),
            us(c.run_latency.p99)
        );
    }
    for w in &r.workers {
        eprintln!(
            "  worker {:>2} cpu={:<3} combining={:>13} client_polls={:>8} bystander_polls={:>8} bystander p50={:.1}us p99={:.1}us chains n={} max={} steals={} balance={} parks={}",
            w.worker,
            w.cpu.map_or("-".to_string(), |c| c.to_string()),
            w.combining_cycles,
            w.client_polls,
            w.bystander_polls,
            us(w.bystander_latency.p50),
            us(w.bystander_latency.p99),
            w.chain_lengths.count,
            w.chain_lengths.max,
            w.steals,
            w.balance_steals,
            w.parks
        );
    }
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map_or("null".to_string(), |x| format!("{x:.4}"))
}
