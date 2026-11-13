//! A radix-2 fast Fourier transform, in place.

/// Transforms `re`/`im` (the same power-of-two length) in place.
pub(super) fn transform(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    // Bit reversal.
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let angle = -2.0 * std::f32::consts::PI / size as f32;
        let (step_im, step_re) = angle.sin_cos();
        for start in (0..n).step_by(size) {
            let (mut w_re, mut w_im) = (1.0f32, 0.0f32);
            for k in 0..size / 2 {
                let (a, b) = (start + k, start + k + size / 2);
                let (t_re, t_im) = (re[b] * w_re - im[b] * w_im, re[b] * w_im + im[b] * w_re);
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
                (w_re, w_im) = (w_re * step_re - w_im * step_im, w_re * step_im + w_im * step_re);
            }
        }
        size <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_of_sine_peaks_at_bin() {
        let n = 256;
        let mut re: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 10.0 * i as f32 / n as f32).sin()).collect();
        let mut im = vec![0.0; n];
        transform(&mut re, &mut im);
        let magnitude: Vec<f32> = re.iter().zip(&im).map(|(r, i)| (r * r + i * i).sqrt()).collect();
        let peak = (1..n / 2).max_by(|a, b| magnitude[*a].total_cmp(&magnitude[*b])).unwrap();
        assert_eq!(peak, 10, "ten cycles in the window land in bin ten");
        assert!((magnitude[10] - n as f32 / 2.0).abs() < 0.5, "a unit sine's peak is half the window");
        assert!(magnitude[30] < 1e-2 && magnitude[3] < 1e-2, "and the other bins are empty");
    }

    #[test]
    fn constant_signal_is_all_in_bin_zero() {
        let mut re = vec![1.0f32; 64];
        let mut im = vec![0.0f32; 64];
        transform(&mut re, &mut im);
        assert!((re[0] - 64.0).abs() < 1e-3);
        assert!(re[1..].iter().chain(&im).all(|v| v.abs() < 1e-3));
    }
}
