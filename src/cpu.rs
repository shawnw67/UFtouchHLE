/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CPU emulation.
//!
//! Implemented using the C++ library dynarmic, which is a dynamic recompiler.
//!
//! iPhone OS apps used either ARMv6 or ARMv7-A, which are both 32-bit ISAs.
//! For the moment, only ARMv6 has been tested.

use crate::abi::GuestFunction;
use crate::mem::{ConstPtr, GuestUSize, Mem, MutPtr, Ptr, SafeRead, SafeWrite};

// Import functions from C++
use Podroid_dynarmic_wrapper::*;

type VAddr = u32;
pub type CpuContext = Podroid_DynarmicContext;

/// MHP2G save-analysis: module-level capture logs shared by the read/write
/// hooks and the dump helper (read hook + write hook are in different fns).
static MHP2G_RLOG: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
static MHP2G_WLOG: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
static MHP2G_RLOG_C: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
static MHP2G_WLOG_C: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());

fn Podroid_cpu_read_impl<T: SafeRead + Default>(
    mem: *mut Podroid_Mem,
    addr: VAddr,
    error: *mut bool,
) -> T {
    // MHP2G save-analysis hook: trace reads from the plaintext buffer
    // (0x345dae0..0x3560a20). The inner-encryption routine reads plaintext
    // here and writes ciphertext to 0x313f6000; the read order reveals
    // whether a permutation is applied. Debug-only instrumentation.
    const RTARGET_LO: VAddr = 0x345dae0;
    const RTARGET_HI: VAddr = 0x3560a20;
    // If a panic occurs (probably due to a null-pointer access), we can't let
    // it keep unwinding as it will hit non-Rust stack frames (dynarmic).
    // Instead we catch the unwind and then tell the C++ code a problem occurred
    // so it can immediately halt CPU execution and then panic itself, now
    // with only Rust stack frames to worry about and with CPU state information
    // available that's useful for debugging.
    //
    // TODO: Disable this in debug mode? This relies on dynarmic's
    // check_halt_on_memory_access option which surely has a significant
    // performance impact.
    //
    // I'm not sure if this actually is unwind-safe, but considering
    // the emulator will crash anyway, maybe this is okay.
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: ConstPtr<T> = Ptr::from_bits(addr);
        mem.read(ptr)
    }));
    unsafe {
        error.write(res.is_err());
    }
    let value = res.unwrap_or_default();
    let research = crate::save_research::enabled();
    if research && (RTARGET_LO..RTARGET_HI).contains(&addr) {
        unsafe {
            let mut log = MHP2G_RLOG.lock().unwrap();
            if log.len() < 2000000 {
                let vbytes = std::slice::from_raw_parts(
                    (&value as *const T) as *const u8,
                    std::mem::size_of::<T>(),
                );
                let mut v32 = 0u32;
                for (i, b) in vbytes.iter().enumerate().take(4) {
                    v32 |= (*b as u32) << (8 * i);
                }
                log.push((addr, v32));
            }
        }
    } else if research && (0x313f0000..0x31570000).contains(&addr) {
        unsafe {
            let mut log = MHP2G_RLOG_C.lock().unwrap();
            if log.len() < 3000000 {
                let vbytes = std::slice::from_raw_parts(
                    (&value as *const T) as *const u8,
                    std::mem::size_of::<T>(),
                );
                let mut v32 = 0u32;
                for (i, b) in vbytes.iter().enumerate().take(4) {
                    v32 |= (*b as u32) << (8 * i);
                }
                log.push((addr, v32));
            }
        }
    }
    value
}

fn Podroid_cpu_write_impl<T: SafeWrite>(mem: *mut Podroid_Mem, addr: VAddr, value: T) -> bool {
    // MHP2G save-analysis hook: trace writes into the inner-cipher output
    // buffer (0x313f6000..0x31561000 + ciphertext staging) and the main
    // save buffer (0x345dae0..0x3560a20). Debug-only instrumentation.
    const WTARGETS: [(VAddr, VAddr); 2] = [(0x313f0000, 0x31570000), (0x345dae0, 0x3560a20)];
    let mut hit = false;
    for (lo, hi) in WTARGETS {
        if addr >= lo && addr < hi {
            hit = true;
            break;
        }
    }
    if hit && crate::save_research::enabled() {
        unsafe {
            // Split logs: main plaintext buffer writes vs ciphertext staging.
            let main = (0x345dae0..0x3560a20).contains(&addr);
            let vbytes = std::slice::from_raw_parts(
                (&value as *const T) as *const u8,
                std::mem::size_of::<T>(),
            );
            let mut v32 = 0u32;
            for (i, b) in vbytes.iter().enumerate().take(4) {
                v32 |= (*b as u32) << (8 * i);
            }
            if main {
                let mut log = MHP2G_WLOG.lock().unwrap();
                if log.len() < 2000000 {
                    log.push((addr, v32));
                }
            } else {
                let mut log = MHP2G_WLOG_C.lock().unwrap();
                if log.len() < 3000000 {
                    log.push((addr, v32));
                }
            }
        }
    }
    // See comments above about catch_unwind
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: MutPtr<T> = Ptr::from_bits(addr);
        mem.write(ptr, value)
    }));
    res.is_err()
}

// Export functions for use by C++
#[no_mangle]
extern "C" fn Podroid_cpu_read_u8(mem: *mut Podroid_Mem, addr: VAddr, error: *mut bool) -> u8 {
    Podroid_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn Podroid_cpu_read_u16(mem: *mut Podroid_Mem, addr: VAddr, error: *mut bool) -> u16 {
    Podroid_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn Podroid_cpu_read_u32(mem: *mut Podroid_Mem, addr: VAddr, error: *mut bool) -> u32 {
    Podroid_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn Podroid_cpu_read_u64(mem: *mut Podroid_Mem, addr: VAddr, error: *mut bool) -> u64 {
    Podroid_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn Podroid_cpu_write_u8(mem: *mut Podroid_Mem, addr: VAddr, value: u8) -> bool {
    Podroid_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn Podroid_cpu_write_u16(mem: *mut Podroid_Mem, addr: VAddr, value: u16) -> bool {
    Podroid_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn Podroid_cpu_write_u32(mem: *mut Podroid_Mem, addr: VAddr, value: u32) -> bool {
    Podroid_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn Podroid_cpu_write_u64(mem: *mut Podroid_Mem, addr: VAddr, value: u64) -> bool {
    Podroid_cpu_write_impl(mem, addr, value)
}

pub struct Cpu {
    dynarmic_wrapper: *mut Podroid_DynarmicWrapper,
    /// Copy of the direct memory access pointer used to check it has not
    /// changed. If this is null, direct memory access is not in use.
    direct_memory_access_ptr: *const std::ffi::c_void,
}

impl Drop for Cpu {
    fn drop(&mut self) {
        unsafe { Podroid_DynarmicWrapper_delete(self.dynarmic_wrapper) }
    }
}

/// Why CPU execution ended.
#[derive(Debug)]
pub enum CpuState {
    /// Execution halted due to using up all remaining ticks (normal execution)
    /// or after the single instruction was executed (step execution).
    Normal,
    /// SVC instruction encountered.
    Svc(u32),
    /// An error was encountered.
    Error(CpuError),
}

/// A reason that can cause CPU execution to be interrupted.
#[derive(Debug, Clone, PartialEq)]
pub enum CpuError {
    /// Memory error during execution (probably a null page access).
    MemoryError,
    /// Undefined instruction (perhaps from a GDB software breakpoint).
    UndefinedInstruction,
    /// Breakpoint (`bkpt` instruction).
    Breakpoint,
}

impl Cpu {
    /// The register number of the stack pointer.
    pub const SP: usize = 13;
    /// The register number of the link register.
    #[allow(unused)]
    pub const LR: usize = 14;
    /// The register number of the program counter.
    pub const PC: usize = 15;

    /// When this bit is set in CPSR, the CPU is in Thumb mode.
    pub const CPSR_THUMB: u32 = 0x00000020;

    /// When this bit is set in CPSR, the CPU is in user mode.
    pub const CPSR_USER_MODE: u32 = 0x00000010;

    /// Construct a new CPU instance. If a mutable reference to a [Mem] instance
    /// is provided, direct memory access is enabled, and the CPU instance
    /// becomes bound to that [Mem] instance (subsequent calls must use the same
    /// one).
    pub fn new(direct_memory_access: Option<&mut Mem>) -> Cpu {
        Self::new_for_parallel_use(direct_memory_access, 0, false)
    }

    /// Like [Self::new], for one of several CPUs that run at the same time.
    pub fn new_for_parallel_use(
        direct_memory_access: Option<&mut Mem>,
        processor_id: usize,
        fastmem: bool,
    ) -> Cpu {
        // Null page count is in pages rather than bytes. Mem ensures it is
        // page aligned.
        let null_page_count: usize = direct_memory_access
            .as_ref()
            .map_or(0, |mem| mem.null_segment_size() / 0x1000)
            .try_into()
            .unwrap();
        // Safety: the direct memory access pointer will be retained directly by
        // the dynarmic wrapper and indirectly by cached JIT code, so we must
        // ensure we only execute the CPU while holding a &mut on the Mem object
        // to which that pointer belongs.
        let direct_memory_access_ptr = direct_memory_access
            .map_or(std::ptr::null_mut(), |mem| unsafe {
                mem.direct_memory_access_ptr()
            });
        let dynarmic_wrapper = unsafe {
            Podroid_DynarmicWrapper_new(
                direct_memory_access_ptr,
                null_page_count,
                processor_id,
                fastmem,
            )
        };
        Cpu {
            dynarmic_wrapper,
            direct_memory_access_ptr,
        }
    }

    pub fn regs(&self) -> &[u32; 16] {
        unsafe {
            let ptr = Podroid_DynarmicWrapper_regs_const(self.dynarmic_wrapper);
            &*(ptr as *const [u32; 16])
        }
    }
    pub fn regs_mut(&mut self) -> &mut [u32; 16] {
        unsafe {
            let ptr = Podroid_DynarmicWrapper_regs_mut(self.dynarmic_wrapper);
            &mut *(ptr as *mut [u32; 16])
        }
    }

    /// Dump the registers of the current cpu to the log output.
    /// Silently ignores panics.
    pub fn dump_regs(&self) {
        let regs = self.regs();
        Self::echo_regs(regs);
    }

    pub fn echo_regs(regs: &[u32; 16]) {
        // Silently ignore panics so it's safe to use in contexts where we
        // can't panic.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for row in 0..4 {
                use std::fmt::Write;
                let mut line = String::new();
                for col in 0..4 {
                    let reg_idx = row * 4 + col;
                    match reg_idx {
                        Self::SP => write!(&mut line, "\t SP: "),
                        Self::LR => write!(&mut line, "\t LR: "),
                        Self::PC => write!(&mut line, "\t PC: "),
                        _ if reg_idx <= 9 => write!(&mut line, "\t R{reg_idx}: "),
                        _ => write!(&mut line, "\tR{reg_idx}: "),
                    }
                    .unwrap();
                    write!(&mut line, "{:#010x}", regs[reg_idx]).unwrap();
                }
                echo!("{}", line);
            }
        }));
    }

    pub fn cpsr(&self) -> u32 {
        unsafe { Podroid_DynarmicWrapper_cpsr(self.dynarmic_wrapper) }
    }
    pub fn set_cpsr(&mut self, cpsr: u32) {
        unsafe { Podroid_DynarmicWrapper_set_cpsr(self.dynarmic_wrapper, cpsr) }
    }
    /// The VFP status and control register, e.g. for the rounding mode.
    pub fn fpscr(&self) -> u32 {
        unsafe { Podroid_DynarmicWrapper_fpscr(self.dynarmic_wrapper) }
    }
    pub fn set_fpscr(&mut self, fpscr: u32) {
        unsafe { Podroid_DynarmicWrapper_set_fpscr(self.dynarmic_wrapper, fpscr) }
    }

    /// Swap the current state of the CPU (registers etc) with the state stored
    /// in the context object.
    pub fn swap_context(&mut self, context: &mut CpuContext) {
        unsafe { Podroid_DynarmicWrapper_swap_context(self.dynarmic_wrapper, context) }
    }

    /// Get PC with the Thumb bit appropriately set.
    pub fn pc_with_thumb_bit(&self) -> GuestFunction {
        let pc = self.regs()[Self::PC];
        let thumb = (self.cpsr() & Self::CPSR_THUMB) == Self::CPSR_THUMB;
        GuestFunction::from_addr_and_thumb_flag(pc, thumb)
    }

    /// Set PC and the Thumb flag for executing a guest function. Note that this
    /// does not touch LR.
    pub fn branch(&mut self, new_pc: GuestFunction) {
        self.regs_mut()[Self::PC] = new_pc.addr_without_thumb_bit();
        let cpsr_without_thumb = self.cpsr() & (!Self::CPSR_THUMB);
        self.set_cpsr(cpsr_without_thumb | ((new_pc.is_thumb() as u32) * Self::CPSR_THUMB))
    }

    /// Set the PC and Thumb flag (like [Self::branch]), but also set the LR,
    /// and return the original PC and LR.
    pub fn branch_with_link(
        &mut self,
        new_pc: GuestFunction,
        new_lr: GuestFunction,
    ) -> (GuestFunction, GuestFunction) {
        let old_pc = self.pc_with_thumb_bit();
        let old_lr = GuestFunction::from_addr_with_thumb_bit(self.regs()[Self::LR]);
        self.branch(new_pc);
        self.regs_mut()[Self::LR] = new_lr.addr_with_thumb_bit();
        (old_pc, old_lr)
    }

    /// Clear dynarmic's instruction cache for some range of addresses.
    /// This is of interest to the dynamic linker, which will sometimes rewrite
    /// code.
    pub fn invalidate_cache_range(&mut self, base: VAddr, size: GuestUSize) {
        crate::parallel_jit::invalidate_cache_range(base, size);
        self.invalidate_own_cache_range(base, size);
    }

    /// Like [Self::invalidate_cache_range], without telling other CPUs.
    pub fn invalidate_own_cache_range(&mut self, base: VAddr, size: GuestUSize) {
        unsafe { Podroid_DynarmicWrapper_invalidate_cache_range(self.dynarmic_wrapper, base, size) }
    }

    /// Start CPU execution.
    ///
    /// If `ticks` is [Some], it is used as an abstract time limit. The value
    /// will be reduced proportionately with the amount of ticks expended.
    ///
    /// If `ticks` is [None], the CPU executes only a single instruction. This
    /// is also known as "stepping".
    ///
    /// This will return either because the CPU ran out of time, or because
    /// something else happened which requires attention from the host.
    #[must_use]
    pub fn run_or_step(&mut self, mem: &mut Mem, ticks: Option<&mut u64>) -> CpuState {
        // See ::new() for why this is done.
        if !self.direct_memory_access_ptr.is_null() {
            assert!(self.direct_memory_access_ptr == unsafe { mem.direct_memory_access_ptr() });
        }

        let res = unsafe {
            Podroid_DynarmicWrapper_run_or_step(
                self.dynarmic_wrapper,
                mem as *mut Mem as *mut Podroid_Mem,
                ticks,
            )
        };
        match res {
            -1 => CpuState::Normal,
            -2 => CpuState::Error(CpuError::MemoryError),
            -3 => CpuState::Error(CpuError::UndefinedInstruction),
            -4 => CpuState::Error(CpuError::Breakpoint),
            _ if res < -4 => panic!("Unexpected CPU execution result"),
            svc => CpuState::Svc(svc as u32),
        }
    }
}

/// MHP2G save-analysis helper: clear the capture logs. Called just before
/// the inner-encryption window so the logs contain only cipher operations.
pub fn clear_mhp2g_logs() {
    MHP2G_WLOG.lock().unwrap().clear();
    MHP2G_RLOG.lock().unwrap().clear();
    MHP2G_WLOG_C.lock().unwrap().clear();
    MHP2G_RLOG_C.lock().unwrap().clear();
    log!("[WRITEHOOK] cleared read/write logs for cipher window");
}

/// MHP2G save-analysis helper: dump the captured inner-cipher read/write
/// logs to files. Called from the save-write path after CCCrypt.
pub fn dump_mhp2g_write_log() {
    if !crate::save_research::enabled() {
        return;
    }
    let dump = |name: &str, log: &Vec<(u32, u32)>| {
        let mut out = String::new();
        for (i, (a, v)) in log.iter().enumerate() {
            out.push_str(&format!("{i} {a:x} {v:08x}\n"));
        }
        crate::save_research::write(name, out.as_bytes());
    };
    {
        let wlog = MHP2G_WLOG.lock().unwrap();
        dump("inner_cipher_writes.txt", &wlog);
        log!(
            "[WRITEHOOK] dumped {} main-buffer write entries",
            wlog.len()
        );
    }
    {
        let rlog = MHP2G_RLOG.lock().unwrap();
        dump("inner_cipher_reads.txt", &rlog);
        log!("[WRITEHOOK] dumped {} main-buffer read entries", rlog.len());
    }
    {
        let wlog = MHP2G_WLOG_C.lock().unwrap();
        dump("inner_cipher_writes_313f.txt", &wlog);
        log!("[WRITEHOOK] dumped {} 0x313f write entries", wlog.len());
    }
    {
        let rlog = MHP2G_RLOG_C.lock().unwrap();
        dump("inner_cipher_reads_313f.txt", &rlog);
        log!("[WRITEHOOK] dumped {} 0x313f read entries", rlog.len());
    }
}
