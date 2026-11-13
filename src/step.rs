//! STEP files (.step, .stp): the solids are read and cut into triangles.
//!
//! STEP describes exact surfaces rather than triangles, so the faces are
//! tessellated here, finely enough that curves look round when printed.

use crate::mesh::Mesh;
use glam::Vec3;
use truck_meshalgo::prelude::*;
use truck_stepio::r#in::Table;

/// How many millimetres one length unit of the file is.
fn millimetres_per_unit(text: &str) -> f32 {
    let upper = text.to_ascii_uppercase();
    if upper.contains("CONVERSION_BASED_UNIT('INCH'") || upper.contains("CONVERSION_BASED_UNIT('INCHES'") {
        return 25.4;
    }
    // The length unit is a SI_UNIT with a prefix and METRE.
    for line in upper.lines().filter(|l| l.contains("LENGTH_UNIT") && l.contains("SI_UNIT")) {
        if line.contains(".MILLI.") {
            return 1.0;
        }
        if line.contains(".CENTI.") {
            return 10.0;
        }
        if line.contains(".METRE.") {
            return 1000.0;
        }
    }
    1.0
}

pub fn parse_step(bytes: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    // The STEP reader panics on a few shapes it can't handle; that must not take the program down.
    let result = std::panic::catch_unwind(move || tessellate(&text));
    match result {
        Ok(r) => r,
        Err(_) => Err("this STEP file holds shapes that can't be read yet".into()),
    }
}

fn tessellate(text: &str) -> Result<Mesh, String> {
    let exchange = ruststep::parser::parse(text).map_err(|e| format!("not a readable STEP file: {e}"))?;
    let section = exchange.data.first().ok_or("the STEP file has no data")?;
    let table = Table::from_data_section(section);
    if table.shell.is_empty() {
        return Err("the STEP file holds no solid shapes".into());
    }
    let mut merged = PolygonMesh::default();
    let mut skipped = 0;
    for (_, shell) in table.shell.iter() {
        let Ok(shell) = table.to_compressed_shell(shell) else {
            skipped += 1;
            continue;
        };
        // A rough pass finds the size, then the real one is a thousandth of it.
        let rough = shell.robust_triangulation(0.01).to_polygon();
        let size = rough.bounding_box().diameter();
        if !size.is_finite() || size <= 0.0 {
            skipped += 1;
            continue;
        }
        let mut poly = shell.robust_triangulation(size * 0.001).to_polygon();
        poly.put_together_same_attrs(TOLERANCE * 50.0).remove_degenerate_faces().remove_unused_attrs();
        merged.merge(poly);
    }
    merged.triangulate();
    let scale = millimetres_per_unit(text);
    let v: Vec<Vec3> = merged.positions().iter().map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32) * scale).collect();
    let t: Vec<[u32; 3]> = merged
        .faces()
        .triangle_iter()
        .map(|f| [f[0].pos as u32, f[1].pos as u32, f[2].pos as u32])
        .collect();
    if t.is_empty() {
        return Err(if skipped > 0 {
            "none of the shapes in this STEP file could be read".into()
        } else {
            "the STEP file holds no surfaces".into()
        });
    }
    Ok(Mesh { v, t })
}

#[cfg(test)]
mod tests {
    use super::*;
    use truck_modeling::*;
    use truck_stepio::out;

    /// A STEP file for a box (and optionally a hole through it), made with the same library.
    fn step_for(size: [f64; 3]) -> String {
        let v = builder::vertex(Point3::new(0.0, 0.0, 0.0));
        let e = builder::tsweep(&v, Vector3::new(size[0], 0.0, 0.0));
        let f = builder::tsweep(&e, Vector3::new(0.0, size[1], 0.0));
        let solid: Solid = builder::tsweep(&f, Vector3::new(0.0, 0.0, size[2]));
        to_step(&solid)
    }

    fn to_step(solid: &Solid) -> String {
        let compressed = solid.compress();
        out::CompleteStepDisplay::new(
            out::StepModel::from(&compressed),
            out::StepHeaderDescriptor { organization_system: "test".to_owned(), ..Default::default() },
        )
        .to_string()
    }

    fn volume(m: &Mesh) -> f32 {
        m.t.iter()
            .map(|t| {
                let (a, b, c) = (m.v[t[0] as usize], m.v[t[1] as usize], m.v[t[2] as usize]);
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn a_box_comes_back_with_its_size_and_volume() {
        let text = step_for([20.0, 30.0, 10.0]);
        let m = parse_step(text.as_bytes()).expect("the box should read");
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &m.v {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        assert!(((hi - lo) - Vec3::new(20.0, 30.0, 10.0)).abs().max_element() < 0.01, "{lo:?} {hi:?}");
        assert!((volume(&m).abs() - 6000.0).abs() < 6000.0 * 0.01, "volume {}", volume(&m));
    }

    #[test]
    fn a_cylinder_is_round_and_has_the_right_volume() {
        // A 12 mm radius, 25 mm tall cylinder: a circle swept upwards.
        let v = builder::vertex(Point3::new(12.0, 0.0, 0.0));
        let wire = builder::rsweep(&v, Point3::origin(), Vector3::unit_z(), Rad(7.0));
        let disc = builder::try_attach_plane(&[wire]).expect("a flat face");
        let solid: Solid = builder::tsweep(&disc, Vector3::new(0.0, 0.0, 25.0));
        let m = parse_step(to_step(&solid).as_bytes()).expect("the cylinder should read");
        let expected = std::f32::consts::PI * 12.0 * 12.0 * 25.0;
        assert!((volume(&m).abs() - expected).abs() < expected * 0.01, "volume {} vs {expected}", volume(&m));
        let far = m.v.iter().map(|p| (p.x * p.x + p.y * p.y).sqrt()).fold(0.0, f32::max);
        assert!((far - 12.0).abs() < 0.05, "radius {far}");
    }

    #[test]
    fn units_in_metres_and_inches_become_millimetres() {
        assert_eq!(millimetres_per_unit("#1 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );"), 1.0);
        assert_eq!(millimetres_per_unit("#1 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.) );"), 1000.0);
        assert_eq!(millimetres_per_unit("#1 = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.CENTI.,.METRE.) );"), 10.0);
        assert_eq!(millimetres_per_unit("#9 = CONVERSION_BASED_UNIT('INCH',#8);"), 25.4);
        assert_eq!(millimetres_per_unit("no units here"), 1.0);
    }

    #[test]
    fn rubbish_is_refused_politely() {
        assert!(parse_step(b"this is not a STEP file").is_err());
        assert!(parse_step(b"ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n").is_err());
    }
}
