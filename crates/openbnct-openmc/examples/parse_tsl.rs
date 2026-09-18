// SPDX-License-Identifier: Apache-2.0

//! Smoke-check the MF7/MT4 parser against a real TSL tape.
//! Usage: cargo run -p openbnct-openmc --example parse_tsl -- <tape>

fn main() {
    let path = std::env::args().nth(1).expect("usage: parse_tsl <tape>");
    let text = std::fs::read_to_string(&path).expect("read tape");
    let t0 = std::time::Instant::now();
    let tsl = openbnct_openmc::endf_mf7::parse_tsl(&text).expect("parse");
    println!(
        "ZA={} AWR={:.5} sigma_b={:.4} b  beta_max={:.4}",
        tsl.za, tsl.awr, tsl.sigma_b_barns, tsl.beta_max
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
    // Spot-check S(α,β,T) at room temperature for a few (β,α).
    for (beta, alpha) in [(0.0, 1.0), (1.0, 0.5), (3.0, 0.2)] {
        match tsl.s_at(293.6, beta, alpha) {
            Some(s) => println!("S(alpha={alpha}, beta={beta}, ~293.6K) = {s:.4e}"),
            None => println!("no section near beta={beta}"),
        }
    }
}
