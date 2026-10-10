use genzo_color::*;
use genzo_color::lab::*;

#[test]
fn probe_lab_and_oob() {
    let w = [0.95047, 1.0, 1.08883];
    for xyz in [[0.4124564, 0.2126729, 0.0193339],[0.3575761, 0.7151522, 0.1191920],[0.1804375, 0.0721750, 0.9503041]] {
        println!("{:?}", xyz_to_lab(xyz, w));
    }
    let lin = IccProfile::standard(StandardProfile::LinearBt2020).unwrap();
    for (name, kind, ver) in [("srgb v4", StandardProfile::Srgb, IccVersion::V4_3), ("srgb v2", StandardProfile::Srgb, IccVersion::V2_4), ("adobe v4", StandardProfile::AdobeRgb1998, IccVersion::V4_3), ("adobe v2", StandardProfile::AdobeRgb1998, IccVersion::V2_4), ("lin v4", StandardProfile::LinearBt2020, IccVersion::V4_3)] {
        let dst = IccProfile::standard_with_version(kind, ver).unwrap();
        let t = IccTransform::new(&lin, &dst, RenderingIntent::RelativeColorimetric).unwrap();
        let src = [[-0.001f32, -0.001, -0.001], [-0.2, -0.2, -0.2], [2.0, 2.0, 2.0], [0.5,0.5,0.5]];
        let mut out = [[0.0f32;3];4];
        t.transform(&src, &mut out).unwrap();
        println!("{name}: {out:?}");
    }
    // class mutation
    let srgb = IccProfile::standard(StandardProfile::Srgb).unwrap();
    for cls in [b"link", b"abst", b"nmcl", b"spac", b"scnr", b"prtr"] {
        let mut b = srgb.as_bytes().to_vec();
        b[12..16].copy_from_slice(cls);
        let p = IccProfile::from_bytes(&b);
        println!("{}: {:?}", std::str::from_utf8(cls).unwrap(), p.as_ref().map(|p| p.device_class()));
        if let Ok(p) = p {
            println!("  transform: {:?}", IccTransform::new(&srgb, &p, RenderingIntent::RelativeColorimetric).map(|_| ()));
            println!("  display: {:?}", DisplayProfile::resolve(Some(&b)).unwrap().source());
        }
    }
}
