use super::*;

/// Right-hand side of the primary monitor, clear of the desktop icons.
pub(super) fn default_instances(monitors: &[MonitorInfo], reg: &Registry, card: Card, host: &mut dyn Host) -> Vec<InstanceCfg> {
    let Some(m) = monitors.iter().find(|m| m.x == 0 && m.y == 0).or(monitors.first()) else { return vec![] };
    let logical_w = m.work.2 as f32 / m.scale as f32;
    let size = |id: &str| match reg.get(id) {
        Some(Ok(d)) => card.window_size(d.meta().default_card_size),
        _ => (240.0, 160.0),
    };
    let mut out = Vec::new();
    let mut col = |id: &str, widget: &str, x_from_right: f32, y: f32| {
        let (w, h) = size(widget);
        let mut c = InstanceCfg { id: id.into(), widget: widget.into(), monitor: m.reference(), x: (logical_w - x_from_right - w).max(0.0), y, w, h, ..Default::default() };
        if let Some(Ok(wd)) = reg.get(widget) {
            widgets::set_up_instance(&**wd, &mut c, host);
        }
        out.push(c);
        (w, h)
    };
    let (cw, ch) = col("clock-1", "clock", 8.0, 8.0);
    let (_, dh) = col("digital_clock-1", "digital_clock", 8.0, 8.0 + ch - 20.0);
    let x2 = 8.0 + cw.max(340.0) - 20.0;
    let (lw, lh) = col("icon_list-1", "icon_list", x2, 8.0);
    col("icon_folder-1", "icon_folder", x2 + lw - 20.0, 8.0);
    let _ = (dh, lh);
    out
}
