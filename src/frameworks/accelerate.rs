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
    let address = i128::from(base.to_bits()) + i128::from(stride) * i128::from(index) * 4;
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

// vDSP strides count 32-bit elements and may be negative.
macro_rules! binary_vector {
    ($name:ident, $operation:expr) => {
        fn $name(
            env: &mut Environment,
            a: ConstPtr<f32>,
            sa: i32,
            b: ConstPtr<f32>,
            sb: i32,
            output: MutPtr<f32>,
            so: i32,
            count: u32,
        ) {
            for i in 0..count {
                let x = env.mem.read(at(a, sa, i));
                let y = env.mem.read(at(b, sb, i));
                env.mem.write(at(output, so, i), ($operation)(x, y));
            }
        }
    };
}
binary_vector!(vDSP_vadd, |a: f32, b: f32| a + b);
binary_vector!(vDSP_vsub, |a: f32, b: f32| b - a);
binary_vector!(vDSP_vmul, |a: f32, b: f32| a * b);
binary_vector!(vDSP_vdiv, |a: f32, b: f32| b / a);
binary_vector!(vDSP_vmax, |a: f32, b: f32| a.max(b));
binary_vector!(vDSP_vmin, |a: f32, b: f32| a.min(b));
binary_vector!(vDSP_vdist, |a: f32, b: f32| a.hypot(b));

macro_rules! scalar_vector {
    ($name:ident, $operation:expr) => {
        fn $name(
            env: &mut Environment,
            a: ConstPtr<f32>,
            sa: i32,
            scalar: ConstPtr<f32>,
            output: MutPtr<f32>,
            so: i32,
            count: u32,
        ) {
            let value = env.mem.read(scalar);
            for i in 0..count {
                let x = env.mem.read(at(a, sa, i));
                env.mem.write(at(output, so, i), ($operation)(x, value));
            }
        }
    };
}
scalar_vector!(vDSP_vsmul, |a: f32, b: f32| a * b);
scalar_vector!(vDSP_vsdiv, |a: f32, b: f32| a / b);
scalar_vector!(vDSP_vsadd, |a: f32, b: f32| a + b);

macro_rules! unary_vector {
    ($name:ident, $operation:expr) => {
        fn $name(
            env: &mut Environment,
            input: ConstPtr<f32>,
            si: i32,
            output: MutPtr<f32>,
            so: i32,
            count: u32,
        ) {
            for i in 0..count {
                let value = env.mem.read(at(input, si, i));
                env.mem.write(at(output, so, i), ($operation)(value));
            }
        }
    };
}
unary_vector!(vDSP_vabs, |a: f32| a.abs());
unary_vector!(vDSP_vneg, |a: f32| -a);

fn vDSP_vfill(
    env: &mut Environment,
    scalar: ConstPtr<f32>,
    output: MutPtr<f32>,
    stride: i32,
    count: u32,
) {
    let value = env.mem.read(scalar);
    for i in 0..count {
        env.mem.write(at(output, stride, i), value);
    }
}
fn vDSP_vclr(env: &mut Environment, output: MutPtr<f32>, stride: i32, count: u32) {
    for i in 0..count {
        env.mem.write(at(output, stride, i), 0.0f32);
    }
}
fn vDSP_zvmags(
    env: &mut Environment,
    input: ConstPtr<SplitComplex>,
    si: i32,
    output: MutPtr<f32>,
    so: i32,
    count: u32,
) {
    let split = env.mem.read(input);
    for i in 0..count {
        let real = env.mem.read(at(split.real, si, i));
        let imag = env.mem.read(at(split.imag, si, i));
        env.mem.write(at(output, so, i), real * real + imag * imag);
    }
}
fn vDSP_dotpr(
    env: &mut Environment,
    a: ConstPtr<f32>,
    sa: i32,
    b: ConstPtr<f32>,
    sb: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    let mut total = 0.0;
    for i in 0..count {
        total += env.mem.read(at(a, sa, i)) * env.mem.read(at(b, sb, i));
    }
    env.mem.write(output, total);
}
fn vDSP_sve(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    let mut total = 0.0;
    for i in 0..count {
        total += env.mem.read(at(input, stride, i));
    }
    env.mem.write(output, total);
}
fn vDSP_svesq(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    let mut total = 0.0;
    for i in 0..count {
        let x: f32 = env.mem.read(at(input, stride, i));
        total += x * x;
    }
    env.mem.write(output, total);
}
fn vDSP_meanv(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    vDSP_sve(env, input, stride, output, count);
    if count != 0 {
        let sum = env.mem.read(output);
        env.mem.write(output, sum / count as f32);
    }
}
fn vDSP_rmsqv(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    vDSP_svesq(env, input, stride, output, count);
    if count != 0 {
        let sum: f32 = env.mem.read(output);
        env.mem.write(output, (sum / count as f32).sqrt());
    }
}
fn vDSP_maxv(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    if count == 0 {
        return;
    }
    let mut value: f32 = env.mem.read(input);
    for i in 1..count {
        value = value.max(env.mem.read(at(input, stride, i)));
    }
    env.mem.write(output, value);
}
fn vDSP_minv(
    env: &mut Environment,
    input: ConstPtr<f32>,
    stride: i32,
    output: MutPtr<f32>,
    count: u32,
) {
    if count == 0 {
        return;
    }
    let mut value: f32 = env.mem.read(input);
    for i in 1..count {
        value = value.min(env.mem.read(at(input, stride, i)));
    }
    env.mem.write(output, value);
}
fn vDSP_vma(
    env: &mut Environment,
    a: ConstPtr<f32>,
    sa: i32,
    b: ConstPtr<f32>,
    sb: i32,
    c: ConstPtr<f32>,
    sc: i32,
    output: MutPtr<f32>,
    so: i32,
    count: u32,
) {
    for i in 0..count {
        let x: f32 = env.mem.read(at(a, sa, i));
        let y: f32 = env.mem.read(at(b, sb, i));
        let z: f32 = env.mem.read(at(c, sc, i));
        env.mem.write(at(output, so, i), x * y + z);
    }
}

fn vDSP_normalize(
    env: &mut Environment,
    input: ConstPtr<f32>,
    si: i32,
    output: MutPtr<f32>,
    so: i32,
    mean_out: MutPtr<f32>,
    deviation_out: MutPtr<f32>,
    count: u32,
) {
    if count == 0 {
        return;
    }
    let mut sum = 0.0f64;
    for i in 0..count {
        sum += f64::from(env.mem.read(at(input, si, i)));
    }
    let mean = (sum / count as f64) as f32;
    let mut squares = 0.0f64;
    for i in 0..count {
        let d = f64::from(env.mem.read(at(input, si, i))) - f64::from(mean);
        squares += d * d;
    }
    let deviation = (squares / count as f64).sqrt() as f32;
    if !mean_out.is_null() {
        env.mem.write(mean_out, mean);
    }
    if !deviation_out.is_null() {
        env.mem.write(deviation_out, deviation);
    }
    if !output.is_null() {
        for i in 0..count {
            let value = env.mem.read(at(input, si, i));
            env.mem.write(at(output, so, i), (value - mean) / deviation);
        }
    }
}
fn vDSP_vintb(
    env: &mut Environment,
    a: ConstPtr<f32>,
    sa: i32,
    b: ConstPtr<f32>,
    sb: i32,
    factor: ConstPtr<f32>,
    output: MutPtr<f32>,
    so: i32,
    count: u32,
) {
    let factor: f32 = env.mem.read(factor);
    for i in 0..count {
        let x: f32 = env.mem.read(at(a, sa, i));
        let y: f32 = env.mem.read(at(b, sb, i));
        env.mem.write(at(output, so, i), x + factor * (y - x));
    }
}

// CBLAS negative increments index from the end of the supplied base array.
fn blas_at<T, const MUT: bool>(
    base: crate::mem::Ptr<T, MUT>,
    stride: i32,
    n: u32,
    i: u32,
) -> crate::mem::Ptr<T, MUT> {
    let offset = if stride < 0 {
        i128::from(n - 1 - i) * -i128::from(stride)
    } else {
        i128::from(i) * i128::from(stride)
    };
    let address = i128::from(base.to_bits()) + 4 * offset;
    crate::mem::Ptr::from_bits(u32::try_from(address).expect("BLAS address overflow"))
}
fn cblas_saxpy(
    env: &mut Environment,
    n: i32,
    alpha: f32,
    x: ConstPtr<f32>,
    ix: i32,
    y: MutPtr<f32>,
    iy: i32,
) {
    for i in 0..n.max(0) as u32 {
        let xp = blas_at(x, ix, n as u32, i);
        let yp = blas_at(y, iy, n as u32, i);
        let value: f32 = env.mem.read(xp);
        let old: f32 = env.mem.read(yp);
        env.mem.write(yp, old + alpha * value);
    }
}
fn cblas_scopy(env: &mut Environment, n: i32, x: ConstPtr<f32>, ix: i32, y: MutPtr<f32>, iy: i32) {
    for i in 0..n.max(0) as u32 {
        let value = env.mem.read(blas_at(x, ix, n as u32, i));
        env.mem.write(blas_at(y, iy, n as u32, i), value);
    }
}
fn cblas_sdot(
    env: &mut Environment,
    n: i32,
    x: ConstPtr<f32>,
    ix: i32,
    y: ConstPtr<f32>,
    iy: i32,
) -> f32 {
    let mut value = 0.0;
    for i in 0..n.max(0) as u32 {
        value +=
            env.mem.read(blas_at(x, ix, n as u32, i)) * env.mem.read(blas_at(y, iy, n as u32, i));
    }
    value
}
fn cblas_sscal(env: &mut Environment, n: i32, alpha: f32, x: MutPtr<f32>, ix: i32) {
    if ix <= 0 {
        return;
    }
    for i in 0..n.max(0) as u32 {
        let p = blas_at(x, ix, n as u32, i);
        let value: f32 = env.mem.read(p);
        env.mem.write(p, alpha * value);
    }
}
fn cblas_snrm2(env: &mut Environment, n: i32, x: ConstPtr<f32>, ix: i32) -> f32 {
    if ix <= 0 {
        return 0.0;
    }
    let mut norm = 0.0f32;
    for i in 0..n.max(0) as u32 {
        norm = norm.hypot(env.mem.read(blas_at(x, ix, n as u32, i)));
    }
    norm
}
fn cblas_sgemv(
    env: &mut Environment,
    order: i32,
    trans: i32,
    m: i32,
    n: i32,
    alpha: f32,
    matrix: ConstPtr<f32>,
    lda: i32,
    x: ConstPtr<f32>,
    ix: i32,
    beta: f32,
    y: MutPtr<f32>,
    iy: i32,
) {
    if m <= 0
        || n <= 0
        || lda < if order == 101 { n } else { m }.max(1)
        || ix == 0
        || iy == 0
        || ![101, 102].contains(&order)
        || ![111, 112, 113].contains(&trans)
    {
        return;
    }
    let (rows, cols) = if trans == 111 {
        (m as u32, n as u32)
    } else {
        (n as u32, m as u32)
    };
    for i in 0..rows {
        let mut sum = 0.0;
        for j in 0..if alpha == 0.0 { 0 } else { cols } {
            let (row, col) = if trans == 111 { (i, j) } else { (j, i) };
            let index = if order == 101 {
                u64::from(row) * lda as u64 + u64::from(col)
            } else {
                u64::from(col) * lda as u64 + u64::from(row)
            };
            let index = u32::try_from(index).expect("BLAS matrix index overflow");
            let a: f32 = env.mem.read(at(matrix, 1, index));
            let b: f32 = env.mem.read(blas_at(x, ix, cols, j));
            sum += a * b;
        }
        let out = blas_at(y, iy, rows, i);
        let old: f32 = if beta == 0.0 { 0.0 } else { env.mem.read(out) };
        env.mem.write(out, alpha * sum + beta * old);
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vDSP_normalize(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vintb(_, _, _, _, _, _, _, _)),
    export_c_func!(cblas_saxpy(_, _, _, _, _, _)),
    export_c_func!(cblas_scopy(_, _, _, _, _)),
    export_c_func!(cblas_sdot(_, _, _, _, _)),
    export_c_func!(cblas_sscal(_, _, _, _)),
    export_c_func!(cblas_snrm2(_, _, _)),
    export_c_func!(cblas_sgemv(_, _, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(vDSP_vadd(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vsub(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vmul(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vdiv(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vmax(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vmin(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vdist(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_vsmul(_, _, _, _, _, _)),
    export_c_func!(vDSP_vsdiv(_, _, _, _, _, _)),
    export_c_func!(vDSP_vsadd(_, _, _, _, _, _)),
    export_c_func!(vDSP_vabs(_, _, _, _, _)),
    export_c_func!(vDSP_vneg(_, _, _, _, _)),
    export_c_func!(vDSP_zvmags(_, _, _, _, _)),
    export_c_func!(vDSP_vfill(_, _, _, _)),
    export_c_func!(vDSP_sve(_, _, _, _)),
    export_c_func!(vDSP_svesq(_, _, _, _)),
    export_c_func!(vDSP_meanv(_, _, _, _)),
    export_c_func!(vDSP_rmsqv(_, _, _, _)),
    export_c_func!(vDSP_maxv(_, _, _, _)),
    export_c_func!(vDSP_minv(_, _, _, _)),
    export_c_func!(vDSP_vclr(_, _, _)),
    export_c_func!(vDSP_dotpr(_, _, _, _, _, _)),
    export_c_func!(vDSP_vma(_, _, _, _, _, _, _, _, _)),
    export_c_func!(vDSP_create_fftsetup(_, _)),
    export_c_func!(vDSP_destroy_fftsetup(_)),
    export_c_func!(vDSP_fft_zip(_, _, _, _, _)),
    export_c_func!(vDSP_fft_zop(_, _, _, _, _, _, _)),
    export_c_func!(vDSP_fft_zrip(_, _, _, _, _)),
    export_c_func!(vDSP_ctoz(_, _, _, _, _)),
    export_c_func!(vDSP_ztoc(_, _, _, _, _)),
];
