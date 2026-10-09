use crate::*;
use cadcraft_geom::{Mat3, Vec2, Vec3};

#[test]
fn finite_check_catches_nan_in_any_variant() {
    assert!(line((0.0, 0.0), (10.0, 5.0)).is_finite());
    assert!(EntityKind::Unknown(Unknown { dxf_type: "ACAD_PROXY".into(), tags: vec![] }).is_finite());
    assert!(!EntityKind::Circle(Circle { center: Vec3::ZERO, radius: f64::NAN }).is_finite());
    assert!(!EntityKind::Circle(Circle { center: Vec3::ZERO, radius: f64::INFINITY }).is_finite());
    assert!(!EntityKind::Arc(Arc { center: Vec3::ZERO, radius: 1.0, start: 0.0, end: f64::NAN }).is_finite());
    assert!(!line((0.0, 0.0), (f64::NAN, 5.0)).is_finite());
    assert!(
        !EntityKind::Table(Table {
            insert: Vec3::ZERO,
            col_widths: vec![10.0, f64::NAN],
            row_heights: vec![5.0],
            cells: vec![],
            style: "Standard".into(),
            text_height: 0.18,
            title: false,
            header: false,
        })
        .is_finite()
    );
    assert!(line((1e308, 0.0), (10.0, 5.0)).is_finite(), "extreme but finite values are not corruption");
}

fn line(a: (f64, f64), b: (f64, f64)) -> EntityKind {
    EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })
}

#[test]
fn new_drawing_has_standard_tables() {
    let d = Drawing::new_imperial();
    assert!(d.layer("0").is_some());
    assert!(d.linetype("continuous").is_some());
    assert!(d.text_style("Standard").is_some());
    assert_eq!(d.header.f64("TEXTSIZE", 0.0), 0.2);
    let m = Drawing::new_metric();
    assert!(m.dim_style("ISO-25").is_some());
}

#[test]
fn add_and_extents() {
    let mut d = Drawing::new_imperial();
    d.add(&Space::Model, Common::default(), line((0.0, 0.0), (10.0, 5.0))).unwrap();
    d.add(&Space::Model, Common::default(), EntityKind::Circle(Circle { center: Vec3::new(20.0, 0.0, 0.0), radius: 2.0 })).unwrap();
    let b = d.extents(&Space::Model);
    assert_eq!(b.min, Vec2::new(0.0, -2.0));
    assert_eq!(b.max, Vec2::new(22.0, 5.0));
}

#[test]
fn hidden_layers_excluded_from_extents() {
    let mut d = Drawing::new_imperial();
    let c = Common { layer: "Hidden".into(), ..Default::default() };
    d.add(&Space::Model, c, line((100.0, 100.0), (200.0, 200.0))).unwrap();
    d.add(&Space::Model, Common::default(), line((0.0, 0.0), (1.0, 1.0))).unwrap();
    d.layer_mut("Hidden").unwrap().on = false;
    assert_eq!(d.extents(&Space::Model).max, Vec2::new(1.0, 1.0));
}

#[test]
fn transform_arc_mirror_keeps_shape() {
    let mut k = EntityKind::Arc(Arc { center: Vec3::ZERO, radius: 1.0, start: 0.0, end: std::f64::consts::FRAC_PI_2 });
    k.transform(&Mat3::mirror(Vec2::ZERO, Vec2::Y));
    if let EntityKind::Arc(a) = k {
        let g = cadcraft_geom::Arc::new(a.center.xy(), a.radius, a.start, a.end);
        assert!(g.mid_point().near(Vec2::from_angle(3.0 * std::f64::consts::FRAC_PI_4), 1e-9));
    } else {
        panic!()
    }
}

#[test]
fn insert_bounds_with_cyclic_block_terminate() {
    let mut d = Drawing::new_imperial();
    let mut b = Block::new("A");
    b.entities.push(Entity::new(
        Handle(1),
        EntityKind::Insert(Insert {
            block: "A".into(),
            insert: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: vec![],
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
        }),
    ));
    b.entities.push(Entity::new(Handle(2), line((0.0, 0.0), (1.0, 1.0))));
    d.blocks.insert("A".into(), std::sync::Arc::new(b));
    let h = d
        .add(
            &Space::Model,
            Common::default(),
            EntityKind::Insert(Insert {
                block: "A".into(),
                insert: Vec3::new(5.0, 5.0, 0.0),
                scale: Vec3::new(2.0, 2.0, 1.0),
                rotation: 0.0,
                attribs: vec![],
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
            }),
        )
        .unwrap();
    let e = d.entity(h).unwrap().clone();
    let bb = entity_bounds(&d, &e, 0);
    assert_eq!(bb.max, Vec2::new(7.0, 7.0));
}

#[test]
fn entity_serde_roundtrip() {
    let e = Entity::new(Handle(7), line((1.0, 2.0), (3.0, 4.0)));
    let s = serde_json::to_string(&e).unwrap();
    let back: Entity = serde_json::from_str(&s).unwrap();
    assert_eq!(e, back);
}

#[test]
fn paper_space_entities() {
    let mut d = Drawing::new_imperial();
    let h = d.add(&Space::Paper("Layout1".into()), Common::default(), line((0.0, 0.0), (1.0, 0.0))).unwrap();
    assert_eq!(d.space_of(h), Some(Space::Paper("Layout1".into())));
    assert!(d.remove_entity(h).is_some());
    assert_eq!(d.entity_count(), 0);
}
