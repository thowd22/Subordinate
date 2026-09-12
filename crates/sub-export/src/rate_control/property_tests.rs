//! Property-level tests with an encoder exposing modern vendor controls only.
use super::*;
use gst::subclass::prelude::*;

mod imp {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    #[derive(Default)]
    pub struct TestEncoder {
        quantizers: Mutex<[i32; 3]>,
        quality: Mutex<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TestEncoder {
        const NAME: &'static str = "SubExportQualityTestEncoder";
        type Type = super::TestEncoder;
        type ParentType = gst::Element;
    }

    impl ObjectImpl for TestEncoder {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                let mut specs: Vec<_> = ["qp-const-i", "qp-const-p", "qp-const-b"]
                    .map(|name| {
                        glib::ParamSpecInt::builder(name)
                            .minimum(-1)
                            .maximum(255)
                            .build()
                    })
                    .into_iter()
                    .collect();
                specs.push(
                    glib::ParamSpecDouble::builder("quality")
                        .minimum(0.0)
                        .maximum(1.0)
                        .build(),
                );
                specs
            })
        }

        fn set_property(&self, id: usize, value: &glib::Value, _: &glib::ParamSpec) {
            if id == 4 {
                *self.quality.lock().unwrap() = value.get().unwrap();
            } else {
                self.quantizers.lock().unwrap()[id - 1] = value.get().unwrap();
            }
        }

        fn property(&self, id: usize, _: &glib::ParamSpec) -> glib::Value {
            if id == 4 {
                self.quality.lock().unwrap().to_value()
            } else {
                self.quantizers.lock().unwrap()[id - 1].to_value()
            }
        }
    }
    impl GstObjectImpl for TestEncoder {}
    impl ElementImpl for TestEncoder {}
}

glib::wrapper! {
    pub struct TestEncoder(ObjectSubclass<imp::TestEncoder>) @extends gst::Element, gst::Object;
}

#[test]
fn modern_nvenc_controls_apply_quality_to_inter_frames_too() {
    gst::init().unwrap();
    let encoder: TestEncoder = glib::Object::new();
    let element = encoder.upcast_ref::<gst::Element>();
    let warnings = apply_video_quality(element, "nvav1enc", VideoQuality::Crf { value: 20 });
    // This test double deliberately lacks rc-mode, while modelling the three
    // AV1 properties and the absence of the legacy shared qp-const property.
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("rc-mode"));
    for name in ["qp-const-i", "qp-const-p", "qp-const-b"] {
        assert_eq!(element.property::<i32>(name), 100, "{name}");
    }
}

#[test]
fn videotoolbox_quality_uses_its_floating_point_scale() {
    gst::init().unwrap();
    let encoder: TestEncoder = glib::Object::new();
    let element = encoder.upcast_ref::<gst::Element>();
    for (crf, expected) in [(0, 1.0), (51, 0.0), (20, 1.0 - 20.0 / 51.0)] {
        let warnings = apply_video_quality(element, "vtenc_h264", VideoQuality::Crf { value: crf });
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!((element.property::<f64>("quality") - expected).abs() < f64::EPSILON);
    }
}
