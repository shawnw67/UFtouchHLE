/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Experimental: runs guest code on worker threads, so that guest threads can
//! execute in parallel.
//!
//! Everything else stays on the main thread. A guest thread hands its registers
//! to a worker, which runs guest code until it hits a host call or runs out of
//! ticks. The guest thread's coroutine is blocked meanwhile, so other guest
//! threads can run, and the host call is then handled as usual.
//!
//! Both sides spin briefly while they're waiting, since host calls are far too
//! frequent to sleep between them, and sleep if the wait goes on for longer.

use crate::cpu::{Cpu, CpuContext, CpuState};
use crate::mem::Mem;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Charged for each host call handled by a worker.
const LEAF_CALL_TICKS: u64 = 100;

const IDLE: u8 = 0;
const JOB_READY: u8 = 1;
const RUNNING: u8 = 2;
const DONE: u8 = 3;

/// Whether code invalidations need to reach the workers.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Let workers answer `glGetError`, see [crate::options::Options].
static IGNORE_GL_ERRORS: AtomicBool = AtomicBool::new(false);
static INVALIDATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static INVALIDATIONS: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// Called for any change to guest code, for example linking a function.
pub fn invalidate_cache_range(base: u32, size: u32) {
    if ACTIVE.load(Ordering::Relaxed) {
        let mut invalidations = INVALIDATIONS.lock().unwrap();
        invalidations.push((base, size));
        INVALIDATION_COUNT.store(invalidations.len(), Ordering::Release);
    }
}

/// Thread-specific data for keys and threads with low numbers, shared with
/// `pthread_getspecific` etc so that worker threads can access it.
pub const FAST_TLS_THREADS: usize = 64;
pub const FAST_TLS_KEYS: usize = 64;
#[allow(clippy::declare_interior_mutable_const)]
const TLS_ZERO: AtomicU32 = AtomicU32::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const TLS_ROW_ZERO: [AtomicU32; FAST_TLS_KEYS] = [TLS_ZERO; FAST_TLS_KEYS];
static FAST_TLS: [[AtomicU32; FAST_TLS_KEYS]; FAST_TLS_THREADS] = [TLS_ROW_ZERO; FAST_TLS_THREADS];
static FAST_TLS_KEY_VALID: [AtomicBool; FAST_TLS_KEYS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; FAST_TLS_KEYS]
};

pub fn fast_tls_get(thread: usize, key_index: usize) -> u32 {
    FAST_TLS[thread][key_index].load(Ordering::Relaxed)
}
pub fn fast_tls_set(thread: usize, key_index: usize, value: u32) {
    FAST_TLS[thread][key_index].store(value, Ordering::Relaxed);
}
/// Set up a key slot, or clear it if the key was deleted.
pub fn fast_tls_reset_key(key_index: usize, valid: bool) {
    for thread in &FAST_TLS {
        thread[key_index].store(0, Ordering::Relaxed);
    }
    FAST_TLS_KEY_VALID[key_index].store(valid, Ordering::Relaxed);
}

/// Mutex lock states for mutexes with low IDs, which workers can change.
/// Other mutexes only get used by the main thread.
pub const FAST_MUTEX_COUNT: usize = 4096;
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U64: AtomicU64 = AtomicU64::new(0);
pub static FAST_MUTEXES: [AtomicU64; FAST_MUTEX_COUNT] = [ZERO_U64; FAST_MUTEX_COUNT];
#[allow(clippy::declare_interior_mutable_const)]
const ZERO_U8: AtomicU8 = AtomicU8::new(0);
static FAST_MUTEX_KINDS: [AtomicU8; FAST_MUTEX_COUNT] = [ZERO_U8; FAST_MUTEX_COUNT];
const MUTEX_KIND_INVALID: u8 = 0;
const MUTEX_KIND_OTHER: u8 = 1;
const MUTEX_KIND_RECURSIVE: u8 = 2;

/// Unlocked is 0, or the thread and count if the mutex is locked.
pub fn encode_mutex_lock(thread: usize, count: u32) -> u64 {
    ((thread as u64 + 1) << 32) | count as u64
}
pub fn decode_mutex_lock(word: u64) -> Option<(usize, u32)> {
    (word != 0).then(|| ((word >> 32) as usize - 1, word as u32))
}
/// Tell workers about a new mutex, or that it was destroyed.
pub fn set_fast_mutex_kind(id: u64, valid: bool, recursive: bool) {
    if let Some(kind) = FAST_MUTEX_KINDS.get(id as usize) {
        let value = match (valid, recursive) {
            (false, _) => MUTEX_KIND_INVALID,
            (true, false) => MUTEX_KIND_OTHER,
            (true, true) => MUTEX_KIND_RECURSIVE,
        };
        kind.store(value, Ordering::Relaxed);
    }
}

/// Get a mutex's ID from guest memory, if a worker can use it.
unsafe fn fast_mutex_id(base: *mut u8, mutex: u32) -> Option<usize> {
    if !(NULL_LIMIT..=u32::MAX - 12).contains(&mutex) {
        return None;
    }
    let ptr = base.add(mutex as usize);
    if (ptr as *const u32).read_unaligned() != crate::libc::pthread::mutex::MAGIC_MUTEX {
        return None;
    }
    let id = (ptr.add(4) as *const u64).read_unaligned();
    ((id as usize) < FAST_MUTEX_COUNT).then_some(id as usize)
}

/// Host functions that workers can run themselves, instead of asking the main
/// thread. These need to be simple, and must not touch the `Environment`.
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
enum Leaf {
    Unknown = 0,
    HostOnly,
    ToUpper,
    GetSpecific,
    SetSpecific,
    Memcpy,
    Memset,
    Strcmp,
    Wcslen,
    Umodsi3,
    Udivsi3,
    Ceilf,
    Floorf,
    MutexLock,
    MutexUnlock,
    GlGetError,
    Memcmp,
}
const MAX_LEAF_SVC: usize = 8192;
#[allow(clippy::declare_interior_mutable_const)]
const LEAF_UNKNOWN: AtomicU8 = AtomicU8::new(Leaf::Unknown as u8);
static LEAVES: [AtomicU8; MAX_LEAF_SVC] = [LEAF_UNKNOWN; MAX_LEAF_SVC];

/// Tell workers which function a host call (SVC number) is linked to.
pub fn note_svc(svc: u32, symbol: &str) {
    let Some(slot) = LEAVES.get(svc as usize) else {
        return;
    };
    if slot.load(Ordering::Relaxed) != Leaf::Unknown as u8 {
        return;
    }
    if let Some(index) = GL_CALLS.iter().position(|(name, ..)| *name == symbol) {
        slot.store(LEAF_GL_BASE + index as u8, Ordering::Relaxed);
        return;
    }
    let leaf = match symbol {
        "___toupper" => Leaf::ToUpper,
        "_pthread_getspecific" => Leaf::GetSpecific,
        "_pthread_setspecific" => Leaf::SetSpecific,
        "_memcpy" => Leaf::Memcpy,
        "_memset" => Leaf::Memset,
        "_strcmp" => Leaf::Strcmp,
        "_wcslen" => Leaf::Wcslen,
        "___umodsi3" => Leaf::Umodsi3,
        "___udivsi3" => Leaf::Udivsi3,
        "_ceilf" => Leaf::Ceilf,
        "_floorf" => Leaf::Floorf,
        "_pthread_mutex_lock" => Leaf::MutexLock,
        "_pthread_mutex_unlock" => Leaf::MutexUnlock,
        "_glGetError" if IGNORE_GL_ERRORS.load(Ordering::Relaxed) => Leaf::GlGetError,
        "_memcmp" => Leaf::Memcmp,
        _ => Leaf::HostOnly,
    };
    slot.store(leaf as u8, Ordering::Relaxed);
}

fn leaf_for_svc(svc: u32) -> Leaf {
    let Some(slot) = LEAVES.get(svc as usize) else {
        return Leaf::HostOnly;
    };
    match slot.load(Ordering::Relaxed) {
        x if x == Leaf::ToUpper as u8 => Leaf::ToUpper,
        x if x == Leaf::GetSpecific as u8 => Leaf::GetSpecific,
        x if x == Leaf::SetSpecific as u8 => Leaf::SetSpecific,
        x if x == Leaf::Memcpy as u8 => Leaf::Memcpy,
        x if x == Leaf::Memset as u8 => Leaf::Memset,
        x if x == Leaf::Strcmp as u8 => Leaf::Strcmp,
        x if x == Leaf::Wcslen as u8 => Leaf::Wcslen,
        x if x == Leaf::Umodsi3 as u8 => Leaf::Umodsi3,
        x if x == Leaf::Udivsi3 as u8 => Leaf::Udivsi3,
        x if x == Leaf::Ceilf as u8 => Leaf::Ceilf,
        x if x == Leaf::Floorf as u8 => Leaf::Floorf,
        x if x == Leaf::MutexLock as u8 => Leaf::MutexLock,
        x if x == Leaf::MutexUnlock as u8 => Leaf::MutexUnlock,
        x if x == Leaf::GlGetError as u8 => Leaf::GlGetError,
        x if x == Leaf::Memcmp as u8 => Leaf::Memcmp,
        _ => Leaf::HostOnly,
    }
}

const NULL_LIMIT: u32 = 0x1000;

/// Run a host function on the worker. Returns false to leave it to the main
/// thread, which also handles anything unusual, such as bad pointers.
unsafe fn run_leaf(leaf: Leaf, regs: &mut [u32; 16], base: *mut u8, thread: usize) -> bool {
    let (a, b, c) = (regs[0], regs[1], regs[2]);
    let fine_range = |ptr: u32, len: u32| ptr >= NULL_LIMIT && ptr.checked_add(len).is_some();
    match leaf {
        Leaf::ToUpper => {
            if (b'a' as u32..=b'z' as u32).contains(&a) {
                regs[0] = a - 32;
            }
        }
        Leaf::GetSpecific | Leaf::SetSpecific => {
            let Some(key_index) = (a as usize).checked_sub(1) else {
                return false;
            };
            if key_index >= FAST_TLS_KEYS
                || thread >= FAST_TLS_THREADS
                || !FAST_TLS_KEY_VALID[key_index].load(Ordering::Relaxed)
            {
                return false;
            }
            if leaf == Leaf::GetSpecific {
                regs[0] = fast_tls_get(thread, key_index);
            } else {
                fast_tls_set(thread, key_index, b);
                regs[0] = 0;
            }
        }
        Leaf::Memcpy => {
            if !fine_range(a, c) || !fine_range(b, c) {
                return false;
            }
            std::ptr::copy(base.add(b as usize), base.add(a as usize), c as usize);
        }
        Leaf::Memset => {
            if !fine_range(a, c) {
                return false;
            }
            std::ptr::write_bytes(base.add(a as usize), b as u8, c as usize);
        }
        Leaf::Strcmp => {
            if a < NULL_LIMIT || b < NULL_LIMIT {
                return false;
            }
            let (mut x, mut y) = (a, b);
            let result = loop {
                let (p, q) = (*base.add(x as usize), *base.add(y as usize));
                if p != q || p == 0 {
                    break (p as i32 - q as i32).signum();
                }
                x = x.wrapping_add(1);
                y = y.wrapping_add(1);
            };
            regs[0] = result as u32;
        }
        Leaf::Wcslen => {
            if a < NULL_LIMIT {
                return false;
            }
            let mut len = 0u32;
            while (base.add(a.wrapping_add(len * 4) as usize) as *const u32).read_unaligned() != 0 {
                len += 1;
            }
            regs[0] = len;
        }
        Leaf::Umodsi3 | Leaf::Udivsi3 => {
            if b == 0 {
                return false;
            }
            regs[0] = if leaf == Leaf::Umodsi3 { a % b } else { a / b };
        }
        Leaf::MutexLock => {
            let Some(id) = fast_mutex_id(base, a) else {
                return false;
            };
            let kind = FAST_MUTEX_KINDS[id].load(Ordering::Relaxed);
            if kind == MUTEX_KIND_INVALID {
                return false;
            }
            let word = &FAST_MUTEXES[id];
            match decode_mutex_lock(word.load(Ordering::Relaxed)) {
                None => {
                    let locked = encode_mutex_lock(thread, 1);
                    if word
                        .compare_exchange(0, locked, Ordering::Acquire, Ordering::Relaxed)
                        .is_err()
                    {
                        return false;
                    }
                }
                Some((owner, count)) if owner == thread && kind == MUTEX_KIND_RECURSIVE => {
                    word.store(encode_mutex_lock(thread, count + 1), Ordering::Relaxed);
                }
                // The main thread will block this thread.
                Some(_) => return false,
            }
            regs[0] = 0;
        }
        Leaf::MutexUnlock => {
            let Some(id) = fast_mutex_id(base, a) else {
                return false;
            };
            if FAST_MUTEX_KINDS[id].load(Ordering::Relaxed) == MUTEX_KIND_INVALID {
                return false;
            }
            let word = &FAST_MUTEXES[id];
            match decode_mutex_lock(word.load(Ordering::Relaxed)) {
                Some((owner, 1)) if owner == thread => word.store(0, Ordering::Release),
                Some((owner, count)) if owner == thread => {
                    word.store(encode_mutex_lock(thread, count - 1), Ordering::Release);
                }
                _ => return false,
            }
            regs[0] = 0;
        }
        Leaf::GlGetError => regs[0] = 0,
        Leaf::Memcmp => {
            if !fine_range(a, c) || !fine_range(b, c) {
                return false;
            }
            let x = std::slice::from_raw_parts(base.add(a as usize), c as usize);
            let y = std::slice::from_raw_parts(base.add(b as usize), c as usize);
            regs[0] = match x.cmp(y) {
                std::cmp::Ordering::Less => -1i32 as u32,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
        }
        Leaf::Ceilf => regs[0] = f32::from_bits(a).ceil().to_bits(),
        Leaf::Floorf => regs[0] = f32::from_bits(a).floor().to_bits(),
        Leaf::Unknown | Leaf::HostOnly => return false,
    }
    true
}

/// OpenGL ES functions which just change state and return nothing. Workers
/// queue these, and the main thread runs them, in order, before it next does
/// anything for that guest thread.
#[derive(Clone, Copy, Debug)]
pub enum GlCall {
    ActiveTexture,
    BindBuffer,
    BindTexture,
    BlendColor,
    BlendEquationSeparate,
    BlendFunc,
    BlendFuncSeparate,
    ColorMask,
    CullFace,
    DepthFunc,
    DepthMask,
    Disable,
    DisableVertexAttribArray,
    Enable,
    EnableVertexAttribArray,
    FrontFace,
    PolygonOffset,
    StencilFunc,
    StencilMask,
    StencilOp,
    TexParameteri,
    Uniform1f,
    Uniform1fv,
    Uniform1i,
    Uniform2fv,
    Uniform3fv,
    Uniform4fv,
    UniformMatrix3fv,
    UniformMatrix4fv,
    UseProgram,
    VertexAttribPointer,
}

/// For a function with an array of values: which argument is the pointer, and
/// the size of one element. The argument after `location` is the count.
type GlArray = Option<(usize, u32)>;

const GL_CALLS: &[(&str, GlCall, GlArray)] = &[
    ("_glActiveTexture", GlCall::ActiveTexture, None),
    ("_glBindBuffer", GlCall::BindBuffer, None),
    ("_glBindTexture", GlCall::BindTexture, None),
    ("_glBlendColor", GlCall::BlendColor, None),
    (
        "_glBlendEquationSeparate",
        GlCall::BlendEquationSeparate,
        None,
    ),
    ("_glBlendFunc", GlCall::BlendFunc, None),
    ("_glBlendFuncSeparate", GlCall::BlendFuncSeparate, None),
    ("_glColorMask", GlCall::ColorMask, None),
    ("_glCullFace", GlCall::CullFace, None),
    ("_glDepthFunc", GlCall::DepthFunc, None),
    ("_glDepthMask", GlCall::DepthMask, None),
    ("_glDisable", GlCall::Disable, None),
    (
        "_glDisableVertexAttribArray",
        GlCall::DisableVertexAttribArray,
        None,
    ),
    ("_glEnable", GlCall::Enable, None),
    (
        "_glEnableVertexAttribArray",
        GlCall::EnableVertexAttribArray,
        None,
    ),
    ("_glFrontFace", GlCall::FrontFace, None),
    ("_glPolygonOffset", GlCall::PolygonOffset, None),
    ("_glStencilFunc", GlCall::StencilFunc, None),
    ("_glStencilMask", GlCall::StencilMask, None),
    ("_glStencilOp", GlCall::StencilOp, None),
    ("_glTexParameteri", GlCall::TexParameteri, None),
    ("_glUniform1f", GlCall::Uniform1f, None),
    ("_glUniform1fv", GlCall::Uniform1fv, Some((2, 4))),
    ("_glUniform1i", GlCall::Uniform1i, None),
    ("_glUniform2fv", GlCall::Uniform2fv, Some((2, 8))),
    ("_glUniform3fv", GlCall::Uniform3fv, Some((2, 12))),
    ("_glUniform4fv", GlCall::Uniform4fv, Some((2, 16))),
    (
        "_glUniformMatrix3fv",
        GlCall::UniformMatrix3fv,
        Some((3, 36)),
    ),
    (
        "_glUniformMatrix4fv",
        GlCall::UniformMatrix4fv,
        Some((3, 64)),
    ),
    ("_glUseProgram", GlCall::UseProgram, None),
    ("_glVertexAttribPointer", GlCall::VertexAttribPointer, None),
];
/// Values of [LEAVES] from here on are indices into [GL_CALLS].
const LEAF_GL_BASE: u8 = 64;
/// Larger arrays are left to the main thread.
const MAX_GL_ARRAY_SIZE: u32 = 0x4000;

/// A queued function call. For arrays, the pointer argument is an offset into
/// the job's data.
pub struct QueuedGlCall {
    pub call: GlCall,
    pub args: [u32; 6],
}

/// Add a function call to the queue. Returns false to leave it to the main
/// thread, which is always safe, since the queue gets emptied first.
unsafe fn queue_gl_call(
    index: usize,
    regs: &[u32; 16],
    base: *mut u8,
    calls: &mut Vec<QueuedGlCall>,
    data: &mut Vec<u8>,
) -> bool {
    let (_, call, array) = GL_CALLS[index];
    let mut args = [regs[0], regs[1], regs[2], regs[3], 0, 0];
    if matches!(call, GlCall::VertexAttribPointer) {
        // The remaining arguments are on the stack.
        let sp = regs[Cpu::SP];
        args[4] = (base.add(sp as usize) as *const u32).read_unaligned();
        args[5] = (base.add(sp as usize + 4) as *const u32).read_unaligned();
    }
    if let Some((pointer_arg, element_size)) = array {
        let (count, pointer) = (args[1] as i32, args[pointer_arg]);
        if count < 0 || count as u32 > MAX_GL_ARRAY_SIZE / element_size {
            return false;
        }
        let size = count as u32 * element_size;
        if pointer < NULL_LIMIT || pointer.checked_add(size).is_none() {
            return false;
        }
        args[pointer_arg] = data.len() as u32;
        data.extend_from_slice(std::slice::from_raw_parts(
            base.add(pointer as usize),
            size as usize,
        ));
    }
    calls.push(QueuedGlCall { call, args });
    true
}

/// The result of a guest code run. The GL calls need to run before anything
/// else happens for the guest thread.
pub struct JobResult {
    pub context: Box<CpuContext>,
    pub ticks: u64,
    pub state: CpuState,
    pub gl_calls: Vec<QueuedGlCall>,
    pub gl_data: Vec<u8>,
}

struct Job {
    thread: usize,
    gl_calls: Vec<QueuedGlCall>,
    gl_data: Vec<u8>,
    context: Box<CpuContext>,
    ticks: u64,
    mem: *mut Mem,
    state: Option<CpuState>,
}

struct Worker {
    phase: AtomicU8,
    job: UnsafeCell<Option<Job>>,
    /// Set by the worker thread before it looks at `phase`.
    sleeper: OnceLock<Sleeper>,
}
// SAFETY: `phase` decides which thread may access `job`.
unsafe impl Sync for Worker {}
unsafe impl Send for Worker {}

/// How long to spin for an event before sleeping. Most host calls are far
/// shorter than this, but waiting for a long one shouldn't cost a CPU core.
const SPIN_LIMIT: u32 = 4000;

/// Lets a spinning thread sleep until the other side has something for it.
struct Sleeper {
    thread: std::thread::Thread,
    sleeping: AtomicBool,
}
impl Sleeper {
    fn current() -> Sleeper {
        Sleeper {
            thread: std::thread::current(),
            sleeping: AtomicBool::new(false),
        }
    }
    /// Sleep unless `ready` becomes true. This can return early.
    fn sleep(&self, ready: impl Fn() -> bool, timeout: std::time::Duration) {
        self.sleeping.store(true, Ordering::SeqCst);
        if !ready() {
            std::thread::park_timeout(timeout);
        }
        self.sleeping.store(false, Ordering::SeqCst);
    }
    fn wake(&self) {
        if self.sleeping.load(Ordering::SeqCst) {
            self.thread.unpark();
        }
    }
}

struct WorkerCpu(Cpu);
// SAFETY: only the worker thread uses this after creating it.
unsafe impl Send for WorkerCpu {}

pub struct Pool {
    workers: Vec<Arc<Worker>>,
    main_thread: Arc<Sleeper>,
    idle_spins: u32,
}

impl Pool {
    pub fn new(mem: &mut Mem, count: usize, ignore_gl_errors: bool) -> Pool {
        ACTIVE.store(true, Ordering::Relaxed);
        IGNORE_GL_ERRORS.store(ignore_gl_errors, Ordering::Relaxed);
        let main_thread = Arc::new(Sleeper::current());
        let workers = (0..count)
            .map(|i| {
                let worker = Arc::new(Worker {
                    phase: AtomicU8::new(IDLE),
                    job: UnsafeCell::new(None),
                    sleeper: OnceLock::new(),
                });
                // The main CPU has processor ID 0.
                let cpu = WorkerCpu(Cpu::new_with_processor_id(Some(&mut *mem), i + 1));
                let thread_worker = worker.clone();
                let thread_main = main_thread.clone();
                std::thread::Builder::new()
                    .name(format!("GuestJit{i}"))
                    .spawn(move || worker_main(cpu, thread_worker, thread_main))
                    .unwrap();
                worker
            })
            .collect();
        Pool {
            workers,
            main_thread,
            idle_spins: 0,
        }
    }

    /// Start running guest code. Gives the context back if no worker is free.
    pub fn submit(
        &self,
        thread: usize,
        context: Box<CpuContext>,
        ticks: u64,
        mem: &mut Mem,
    ) -> Result<usize, Box<CpuContext>> {
        let Some(index) = self
            .workers
            .iter()
            .position(|worker| worker.phase.load(Ordering::Acquire) == IDLE)
        else {
            return Err(context);
        };
        let worker = &self.workers[index];
        // SAFETY: the worker doesn't touch this until it sees JOB_READY.
        unsafe {
            *worker.job.get() = Some(Job {
                thread,
                gl_calls: Vec::new(),
                gl_data: Vec::new(),
                context,
                ticks,
                mem,
                state: None,
            });
        }
        worker.phase.store(JOB_READY, Ordering::Release);
        if let Some(sleeper) = worker.sleeper.get() {
            sleeper.wake();
        }
        Ok(index)
    }

    pub fn is_done(&self, index: usize) -> bool {
        self.workers[index].phase.load(Ordering::Acquire) == DONE
    }

    fn any_done(&self) -> bool {
        (0..self.workers.len()).any(|index| self.is_done(index))
    }

    /// Wait a moment for a worker to finish. The scheduler needs to look again
    /// soon even then, because other threads could become ready meanwhile.
    pub fn wait_for_worker(&mut self) {
        self.idle_spins += 1;
        if self.idle_spins < SPIN_LIMIT {
            std::hint::spin_loop();
        } else {
            let timeout = std::time::Duration::from_micros(500);
            self.main_thread.sleep(|| self.any_done(), timeout);
        }
    }

    /// A worker finished, or the scheduler has something else to do.
    pub fn reset_wait(&mut self) {
        self.idle_spins = 0;
    }

    pub fn take_result(&self, index: usize) -> JobResult {
        let worker = &self.workers[index];
        assert!(worker.phase.load(Ordering::Acquire) == DONE);
        // SAFETY: the worker doesn't touch this until it sees JOB_READY.
        let job = unsafe { (*worker.job.get()).take().unwrap() };
        worker.phase.store(IDLE, Ordering::Release);
        JobResult {
            context: job.context,
            ticks: job.ticks,
            state: job.state.unwrap(),
            gl_calls: job.gl_calls,
            gl_data: job.gl_data,
        }
    }
}

fn worker_main(cpu: WorkerCpu, worker: Arc<Worker>, main_thread: Arc<Sleeper>) {
    let WorkerCpu(mut cpu) = cpu;
    let mut applied_invalidations = 0;
    let mut idle_spins = 0u32;
    worker.sleeper.set(Sleeper::current()).ok().unwrap();
    let sleeper = worker.sleeper.get().unwrap();
    let job_ready = || worker.phase.load(Ordering::Acquire) == JOB_READY;
    loop {
        if !job_ready() {
            idle_spins += 1;
            if idle_spins < SPIN_LIMIT {
                std::hint::spin_loop();
            } else {
                sleeper.sleep(job_ready, std::time::Duration::from_millis(5));
            }
            continue;
        }
        idle_spins = 0;
        worker.phase.store(RUNNING, Ordering::Relaxed);

        if INVALIDATION_COUNT.load(Ordering::Acquire) != applied_invalidations {
            let invalidations = INVALIDATIONS.lock().unwrap();
            for &(base, size) in &invalidations[applied_invalidations..] {
                cpu.invalidate_own_cache_range(base, size);
            }
            applied_invalidations = invalidations.len();
        }

        // SAFETY: the main thread doesn't touch this until it sees DONE.
        let job = unsafe { (*worker.job.get()).as_mut().unwrap() };
        cpu.swap_context(&mut job.context);
        // SAFETY: guest memory stays mapped, and generated code only uses the
        // direct access pointer. Guest code on different threads racing with
        // each other is the same as on a real multi-core device.
        let mem = unsafe { &mut *job.mem };
        let base = unsafe { mem.direct_memory_access_ptr() }.cast::<u8>();
        let state = loop {
            let state = cpu.run_or_step(mem, Some(&mut job.ticks));
            let CpuState::Svc(svc) = state else {
                break state;
            };
            let leaf = LEAVES
                .get(svc as usize)
                .map_or(0, |leaf| leaf.load(Ordering::Relaxed));
            let handled = if leaf >= LEAF_GL_BASE {
                let index = (leaf - LEAF_GL_BASE) as usize;
                let (calls, data) = (&mut job.gl_calls, &mut job.gl_data);
                unsafe { queue_gl_call(index, cpu.regs(), base, calls, data) }
            } else {
                unsafe { run_leaf(leaf_for_svc(svc), cpu.regs_mut(), base, job.thread) }
            };
            if !handled {
                break state;
            }
            job.ticks = job.ticks.saturating_sub(LEAF_CALL_TICKS);
            if job.ticks == 0 {
                break CpuState::Normal;
            }
        };
        cpu.swap_context(&mut job.context);
        job.state = Some(state);
        worker.phase.store(DONE, Ordering::Release);
        main_thread.wake();
    }
}
