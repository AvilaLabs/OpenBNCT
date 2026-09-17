//! Diagnostic: dump pointwise cross sections from a processed OpenMC
//! ENDF/B HDF5 file at representative energies — used to spot-check the
//! declared multigroup collapse values (e.g. the FiR 1 three-group
//! fixtures under `validation/`). Usage: dump_xs <file.h5> <nuclide>
//! <energy_eV>...

use hdf5_pure::File;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let file = File::open(&args[1]).unwrap();
    let nuc = args[2].clone();
    let targets: Vec<f64> = args[3..].iter().map(|s| s.parse().unwrap()).collect();

    let nuc_g = file.group(&format!("/{nuc}")).unwrap();
    let egrid: Vec<f64> = nuc_g
        .group("energy")
        .unwrap()
        .dataset("294K")
        .unwrap()
        .read_f64()
        .unwrap();
    println!(
        "energy grid: {} points, {:.3e} .. {:.3e} eV",
        egrid.len(),
        egrid[0],
        egrid[egrid.len() - 1]
    );

    let reactions = nuc_g.group("reactions").unwrap();
    let mut rnames = reactions.groups().unwrap();
    rnames.sort();
    for rname in &rnames {
        let rg = reactions.group(rname).unwrap();
        let Ok(tg) = rg.group("294K") else { continue };
        let Ok(xs_ds) = tg.dataset("xs") else {
            continue;
        };
        let xs: Vec<f64> = xs_ds.read_f64().unwrap();
        for &e in &targets {
            let i = egrid
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    ((*a / e).ln().abs())
                        .partial_cmp(&((*b / e).ln().abs()))
                        .unwrap()
                })
                .map(|(i, _)| i)
                .unwrap();
            if i < xs.len() {
                println!(
                    "{rname}: xs({e:.4}eV) = {:.4} b (grid E={:.4} eV, n={})",
                    xs[i],
                    egrid[i],
                    xs.len()
                );
            } else {
                println!("{rname}: xs({e:.4}eV) n/a (len {} < idx {i})", xs.len());
            }
        }
    }
}
