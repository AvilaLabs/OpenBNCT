// SPDX-License-Identifier: MIT

//! Smoke-check the MF7/MT4 parser against a real TSL tape.
//! Usage: cargo run -p openbnct-openmc --example parse_tsl -- <tape>

fn main() {
    let path = std::env::args().nth(1).expect("usage: parse_tsl <tape>");
    let text = std::fs::read_to_string(&path).expect("read tape");
    let t0 = std::time::Instant::now();
    let tsl = openbnct_openmc::endf_mf7::parse_tsl(&text).expect("parse");
    println!(
        "ZA={} AWR={:.5} sigma_b={:.4} b  natom={}  bound_sigma_b={:.4} b  beta_max={:.4}",
        tsl.za,
        tsl.awr,
        tsl.sigma_b_barns,
        tsl.natom,
        tsl.bound_sigma_b(),
        tsl.beta_max
    );
    println!(
        "{} sections: {} temps ({}..{} K) x {} betas in {:.2?}",
        tsl.sections.len(),
        tsl.temperatures_k.len(),
        tsl.temperatures_k[0],
        tsl.temperatures_k[tsl.temperatures_k.len() - 1],
        tsl.betas.len(),
        t0.elapsed()
    );
    println!(
        "temps: {:?}",
        &tsl.temperatures_k[..tsl.temperatures_k.len().min(16)]
    );
    let (b0, b1) = (tsl.betas[0], tsl.betas[tsl.betas.len() - 1]);
    let sec = &tsl.sections[0];
    println!(
        "beta grid {b0:.4}..{b1:.2} ({} pts), alpha grid {:.5}..{:.1} ({} pts)",
        tsl.betas.len(),
        sec.alpha[0],
        sec.alpha[sec.alpha.len() - 1],
        sec.alpha.len()
    );
    // Spot-check S(α,β,T) at room temperature for a few (β,α).
    for (beta, alpha) in [(0.0, 1.0), (1.0, 0.5), (3.0, 0.2)] {
        match tsl.s_at(293.6, beta, alpha) {
            Some(s) => println!("S(alpha={alpha}, beta={beta}, ~293.6K) = {s:.4e}"),
            None => println!("no section near beta={beta}"),
        }
    }
    // Kernel normalization: ∫σ(E→E')dE' over the β domain should
    // recover ≈σ_b (the bound XS) for a well-normalized evaluation.
    let kern = openbnct_openmc::endf_mf7::SabKernel::build(&tsl, 293.6).expect("kernel");
    println!(
        "kernel: T={:.2} K  kT={:.5} eV  Δ=β_max·kT={:.3} eV  E_max={:.3} eV",
        kern.temperature_k, kern.kt_ev, kern.delta_ev, kern.emax_ev
    );
    for &e in &[0.025_f64, 0.05, 0.1, 0.4, 1.0, 5.0] {
        let (lo, hi) = (0.0_f64, e + kern.delta_ev);
        let n = 4000;
        let step = (hi - lo) / n as f64;
        let mut total = 0.0;
        for i in 0..n {
            total += kern.sigma0_de(e, lo + (i as f64 + 0.5) * step) * step;
        }
        println!(
            "E={e:>6.3} eV: ∫σ(E→E')dE' = {total:8.4} b  (σ_b={:.4})",
            kern.sigma_b_barns
        );
    }
}
