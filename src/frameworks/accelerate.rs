/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Accelerate's single-precision FFT operations for 32-bit guests.

use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/Accelerate.framework/Accelerate",
    aliases: &[
        "/System/Library/Frameworks/vecLib.framework/vecLib",
        "/System/Library/Frameworks/vDSP.framework/vDSP",
    ],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct SplitComplex {
    real: MutPtr<f32>,
    imag: MutPtr<f32>,
}
unsafe impl SafeRead for SplitComplex {}

fn at<T, const MUT: bool>(
    base: crate::mem::Ptr<T, MUT>,
    stride: i32,
    index: u32,
) -> crate::mem::Ptr<T, MUT> {
    let address = i64::from(base.to_bits()) + i64::from(stride) * i64::from(index) * 4;
    crate::mem::Ptr::from_bits(u32::try_from(address).expect("vDSP address overflow"))
}

fn vDSP_create_fftsetup(env: &mut Environment, log2n: u32, radix: i32) -> MutVoidPtr {
    if radix != 0 || log2n > 22 {
        return MutVoidPtr::null();
    }
    env.mem.alloc_and_write(log2n).cast()
}
fn vDSP_destroy_fftsetup(env: &mut Environment, setup: MutVoidPtr) {
    if !setup.is_null() {
        env.mem.free(setup);
    }
}

fn fft(real: &mut [f32], imag: &mut [f32], inverse: bool) {
    let n = real.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }
    let mut span = 2;
    while span <= n {
        let angle = (if inverse { 1.0 } else { -1.0 }) * std::f32::consts::TAU / span as f32;
        let (step_im, step_re) = angle.sin_cos();
        for base in (0..n).step_by(span) {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for offset in 0..span / 2 {
                let left = base + offset;
                let right = left + span / 2;
                let tr = wr * real[right] - wi * imag[right];
                let ti = wr * imag[right] + wi * real[right];
                real[right] = real[left] - tr;
                imag[right] = imag[left] - ti;
                real[left] += tr;
                imag[left] += ti;
                (wr, wi) = (wr * step_re - wi * step_im, wr * step_im + wi * step_re);
            }
        }
        span *= 2;
    }
}

fn valid_setup(env: &mut Environment, setup: MutVoidPtr, log2n: u32, direction: i32) -> bool {
    !setup.is_null()
        && log2n <= 22
        && log2n <= env.mem.read(setup.cast::<u32>())
        && (direction == 1 || direction == -1)
}

fn complex_fft(
    env: &mut Environment,
    input: SplitComplex,
    input_stride: i32,
    output: SplitComplex,
    output_stride: i32,
    n: u32,
    inverse: bool,
) {
    let mut real = Vec::with_capacity(n as usize);
    let mut imag = Vec::with_capacity(n as usize);
    for i in 0..n {
        real.push(env.mem.read(at(input.real, input_stride, i)));
        imag.push(env.mem.read(at(input.imag, input_stride, i)));
    }
    fft(&mut real, &mut imag, inverse);
    for i in 0..n {
        env.mem
            .write(at(output.real, output_stride, i), real[i as usize]);
        env.mem
            .write(at(output.imag, output_stride, i), imag[i as usize]);
    }
}

fn vDSP_fft_zip(
    env: &mut Environment,
    setup: MutVoidPtr,
    data: ConstPtr<SplitComplex>,
    stride: i32,
    log2n: u32,
    direction: i32,
) {
    if data.is_null() || !valid_setup(env, setup, log2n, direction) {
        return;
    }
    let split = env.mem.read(data);
    complex_fft(
        env,
        split,
        stride,
        split,
        stride,
        1 << log2n,
        direction == -1,
    );
}

fn vDSP_fft_zop(
    env: &mut Environment,
    setup: MutVoidPtr,
    input: ConstPtr<SplitComplex>,
    input_stride: i32,
    output: ConstPtr<SplitComplex>,
    output_stride: i32,
    log2n: u32,
    direction: i32,
) {
    if input.is_null() || output.is_null() || !valid_setup(env, setup, log2n, direction) {
        return;
    }
    let input = env.mem.read(input);
    let output = env.mem.read(output);
    complex_fft(
        env,
        input,
        input_stride,
        output,
        output_stride,
        1 << log2n,
        direction == -1,
    );
}

fn vDSP_fft_zrip(
    env: &mut Environment,
    setup: MutVoidPtr,
    data: ConstPtr<SplitComplex>,
    stride: i32,
    log2n: u32,
    direction: i32,
) {
    if data.is_null() || log2n == 0 || !valid_setup(env, setup, log2n, direction) {
        return;
    }
    let n = 1u32 << log2n;
    let half = n / 2;
    let split = env.mem.read(data);
    let mut real = vec![0.0; n as usize];
    let mut imag = vec![0.0; n as usize];
    if direction == 1 {
        for i in 0..half {
            real[(2 * i) as usize] = env.mem.read(at(split.real, stride, i));
            real[(2 * i + 1) as usize] = env.mem.read(at(split.imag, stride, i));
        }
        fft(&mut real, &mut imag, false);
        env.mem.write(at(split.real, stride, 0), 2.0 * real[0]);
        env.mem
            .write(at(split.imag, stride, 0), 2.0 * real[half as usize]);
        for i in 1..half {
            env.mem
                .write(at(split.real, stride, i), 2.0 * real[i as usize]);
            env.mem
                .write(at(split.imag, stride, i), 2.0 * imag[i as usize]);
        }
    } else {
        real[0] = env.mem.read(at(split.real, stride, 0));
        real[half as usize] = env.mem.read(at(split.imag, stride, 0));
        for i in 1..half {
            let re = env.mem.read(at(split.real, stride, i));
            let im: f32 = env.mem.read(at(split.imag, stride, i));
            real[i as usize] = re;
            imag[i as usize] = im;
            real[(n - i) as usize] = re;
            imag[(n - i) as usize] = -im;
        }
        fft(&mut real, &mut imag, true);
        for i in 0..half {
            env.mem
                .write(at(split.real, stride, i), real[(2 * i) as usize]);
            env.mem
                .write(at(split.imag, stride, i), real[(2 * i + 1) as usize]);
        }
    }
}

fn vDSP_ctoz(
    env: &mut Environment,
    input: ConstPtr<f32>,
    input_stride: i32,
    output: ConstPtr<SplitComplex>,
    output_stride: i32,
    count: u32,
) {
    let split = env.mem.read(output);
    for i in 0..count {
        let real = env.mem.read(at(input, input_stride, i));
        let imag = env.mem.read(at(input, input_stride, i) + 1);
        env.mem.write(at(split.real, output_stride, i), real);
        env.mem.write(at(split.imag, output_stride, i), imag);
    }
}
fn vDSP_ztoc(
    env: &mut Environment,
    input: ConstPtr<SplitComplex>,
    input_stride: i32,
    output: MutPtr<f32>,
    output_stride: i32,
    count: u32,
) {
    let split = env.mem.read(input);
    for i in 0..count {
        let real = env.mem.read(at(split.real, input_stride, i));
        let imag = env.mem.read(at(split.imag, input_stride, i));
        env.mem.write(at(output, output_stride, i), real);
        env.mem.write(at(output, output_stride, i) + 1, imag);
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vDSP_create_fftsetup(_, _)),
    export_c_func!(vDSP_destroy_fftsetup(_)),
    export_c_func!(vDSP_fft_zip(_, _, _, _, _)),
    export_c_func!(vDSP_fft_zop(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_fft_zrip(_, _, _, _, _)),
    export_c_func!(vDSP_ctoz(_, _, _, _, _)),
    export_c_func!(vDSP_ztoc(_, _, _, _, _)),
];
