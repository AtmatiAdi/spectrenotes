//! Kreska z dokumentu -> odcinki do narysowania. Ta sama sciezka co dla
//! kreski rysowanej na zywo, wiec wypalona kreska wyglada identycznie jak mokra.

use spectre_ink::{InkConfig, Segment, StrokeBuilder};
use spectre_proto::StrokeData;

pub fn stroke_segments(data: &StrokeData, ink: &InkConfig, out: &mut Vec<Segment>) {
    let cfg = InkConfig {
        base_width: data.base_width,
        ..*ink
    };
    let mut b = StrokeBuilder::new(cfg);
    for s in &data.samples {
        b.push(*s);
    }
    b.finish(out);
    if data.samples.len() == 1 {
        // Kropka: pojedyncza probka nie daje odcinka, a kropka to tez kreska.
        let p = spectre_ink::Point::new(data.samples[0].x, data.samples[0].y);
        out.push(Segment {
            a: p,
            b: p,
            width: spectre_ink::curve::width_for(
                cfg.curve,
                data.samples[0].pressure,
                cfg.base_width,
                cfg.min_width_ratio,
            ),
        });
    }
}
