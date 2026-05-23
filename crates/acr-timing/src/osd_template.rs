//! RTSS OSD line templates: `{variables}` plus literal RTSS hypertext (`<C=…>`, `<P4>`, …).

use crate::delta_display::DeltaColorStyle;

/// Built-in layouts (used when `sector_line` / `finish_line` are omitted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdTemplatePreset {
    Default,
    Compact,
    /// Live line: delta only; completed sector line stays readable.
    Minimal,
    Custom,
}

impl OsdTemplatePreset {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" | "type2" => Self::Compact,
            "minimal" | "type1" => Self::Minimal,
            "custom" => Self::Custom,
            _ => Self::Default,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OsdTemplateConfig {
    pub preset: OsdTemplatePreset,
    /// Completed sector (upper OSD line / carousel).
    pub sector_line: String,
    /// Active sector while driving (lower OSD line).
    pub live_sector_line: String,
    pub sub_slot: String,
    pub finish_line: String,
    pub max_sub_slots: usize,
    /// RTSS `<S=…>` font scale (percent) for live Δ line only; 0 = default size.
    pub live_delta_font_scale: u32,
}

impl Default for OsdTemplateConfig {
    fn default() -> Self {
        OsdTemplateConfigFile::default().to_runtime()
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct OsdTemplateConfigFile {
    /// `default` | `compact` | `custom` (ignored if `sector_line` is set).
    #[serde(default = "default_preset")]
    pub preset: String,
    /// Completed main-sector line (detail / carousel).
    pub sector_line: Option<String>,
    /// Active sector while timing runs; defaults from `preset` if omitted.
    pub live_sector_line: Option<String>,
    /// One sub gate; repeated for `{subs}` (last `max_sub_slots` only).
    #[serde(default = "default_sub_slot")]
    pub sub_slot: String,
    pub finish_line: Option<String>,
    #[serde(default = "default_max_sub_slots")]
    pub max_sub_slots: usize,
    /// Live Δ font size for RTSS (`S=` percent, e.g. 150). 0 = normal (also after finish).
    pub live_delta_font_scale: Option<u32>,
}

fn default_preset() -> String {
    "default".into()
}
fn default_sub_slot() -> String {
    "[{time:time}]".into()
}
fn default_max_sub_slots() -> usize {
    8
}

impl Default for OsdTemplateConfigFile {
    fn default() -> Self {
        Self {
            preset: default_preset(),
            sector_line: None,
            live_sector_line: None,
            sub_slot: default_sub_slot(),
            finish_line: None,
            max_sub_slots: default_max_sub_slots(),
            live_delta_font_scale: None,
        }
    }
}

/// Wrap OSD text in RTSS font scale tags (percent, e.g. 150). No-op when `scale_percent` is 0 or not RTSS.
///
/// RTSS syntax: `<S=150>…<S>` — reset is `<S>`, not `<S=>` (see guru3D / OverlayEditor hypertext).
pub fn wrap_rtss_font_scale(text: &str, scale_percent: u32, rtss: bool) -> String {
    if !rtss || scale_percent == 0 || text.is_empty() {
        return text.to_string();
    }
    format!("<S={scale_percent}>{text}<S>")
}

impl OsdTemplateConfigFile {
    pub fn to_runtime(&self) -> OsdTemplateConfig {
        let preset = OsdTemplatePreset::parse(&self.preset);
        let live_sector_line = self
            .live_sector_line
            .clone()
            .unwrap_or_else(|| match preset {
                OsdTemplatePreset::Minimal => minimal_live_sector_line(),
                _ => default_live_sector_line(),
            });
        let (sector_line, finish_line) = match preset {
            OsdTemplatePreset::Custom if self.sector_line.is_some() => (
                self.sector_line.clone().unwrap(),
                self.finish_line.clone().unwrap_or_else(default_finish_line),
            ),
            OsdTemplatePreset::Compact => (
                self.sector_line
                    .clone()
                    .unwrap_or_else(compact_sector_line),
                self.finish_line
                    .clone()
                    .unwrap_or_else(compact_finish_line),
            ),
            OsdTemplatePreset::Minimal => (
                self.sector_line
                    .clone()
                    .unwrap_or_else(minimal_completed_sector_line),
                self.finish_line
                    .clone()
                    .unwrap_or_else(minimal_finish_line),
            ),
            _ => (
                self.sector_line
                    .clone()
                    .unwrap_or_else(default_sector_line),
                self.finish_line
                    .clone()
                    .unwrap_or_else(default_finish_line),
            ),
        };
        let live_delta_font_scale = self.live_delta_font_scale.unwrap_or_else(|| match preset {
            OsdTemplatePreset::Minimal => 150,
            _ => 0,
        });
        OsdTemplateConfig {
            preset,
            sector_line,
            live_sector_line,
            sub_slot: self.sub_slot.clone(),
            finish_line,
            max_sub_slots: self.max_sub_slots.max(1),
            live_delta_font_scale,
        }
    }
}

pub fn default_sector_line() -> String {
    "S{sector}: {cum_delta_colored} {subs} ref: {ref:time} tot: {tot:time}".into()
}

pub fn compact_sector_line() -> String {
    "S{sector} {cum_delta_colored} {subs}".into()
}

/// While driving: only sector cumulative Δ (no S#, subs, ref, tot).
pub fn minimal_live_sector_line() -> String {
    "{cum_delta_colored}".into()
}

pub fn default_live_sector_line() -> String {
    "S{sector}: {cum_delta_colored} {subs} ref: {ref:time} tot: {tot:time}".into()
}

/// After a sector ends (carousel / upper line): sector label + Δ + total.
pub fn minimal_completed_sector_line() -> String {
    "S{sector} {cum_delta_colored} tot: {tot:time}".into()
}

pub fn minimal_finish_line() -> String {
    "{delta_colored}".into()
}

pub fn default_finish_line() -> String {
    "Track completed  cum {cum_tot:time}  ref {ref_tot:time}  {delta_colored}".into()
}

pub fn compact_finish_line() -> String {
    "Done {cum_tot:time} {delta_colored}".into()
}

#[derive(Debug, Clone)]
pub struct SubSlotCtx {
    pub time_sec: Option<f64>,
    pub delta_sec: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct SectorLineCtx {
    pub sector_index: u32,
    pub cum_delta_sec: f64,
    pub tot_sec: f64,
    pub reference_tot_sec: Option<f64>,
    pub incomplete: bool,
    pub subs: Vec<SubSlotCtx>,
}

#[derive(Debug, Clone)]
pub struct FinishLineCtx {
    pub cum_tot_sec: f64,
    pub ref_tot_sec: f64,
    pub cum_delta_sec: f64,
}

pub fn format_duration(sec: f64) -> String {
    if !sec.is_finite() || sec < 0.0 {
        return "--:--.--".to_string();
    }
    let total_cs = (sec * 100.0).round() as u64;
    let cs = total_cs % 100;
    let total_s = total_cs / 100;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{cs:02}")
    } else {
        format!("{m}:{s:02}.{cs:02}")
    }
}

fn format_number(v: f64, spec: &str) -> String {
    if !v.is_finite() {
        return "--".into();
    }
    if spec == "time" || spec.is_empty() {
        return format_duration(v);
    }
    if let Some(prec) = spec.strip_prefix('.') {
        if let Ok(n) = prec.parse::<usize>() {
            return format!("{:.*}", n, v);
        }
    }
    if spec.starts_with('+') {
        let prec = spec
            .get(1..)
            .and_then(|s| s.strip_prefix('.'))
            .and_then(|p| p.parse::<usize>().ok())
            .unwrap_or(3);
        let sign = if v >= 0.0 { "+" } else { "" };
        return format!("{sign}{:.*}", prec, v);
    }
    format!("{v}")
}

fn expand_subs(
    template: &str,
    ctx: &SectorLineCtx,
    cfg: &OsdTemplateConfig,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let n = ctx.subs.len();
    let start = n.saturating_sub(cfg.max_sub_slots);
    let mut parts = Vec::new();
    for sub in &ctx.subs[start..n] {
        parts.push(render_template_inner(
            template,
            &SectorLineCtx {
                sector_index: ctx.sector_index,
                cum_delta_sec: ctx.cum_delta_sec,
                tot_sec: ctx.tot_sec,
                reference_tot_sec: ctx.reference_tot_sec,
                incomplete: ctx.incomplete,
                subs: vec![sub.clone()],
            },
            cfg,
            rtss,
            delta_style,
            true,
        ));
    }
    parts.join(" ")
}

/// Replace `{var}` and `{var:spec}`; `{subs}` expands `sub_slot` template.
pub fn render_template(
    template: &str,
    ctx: &SectorLineCtx,
    cfg: &OsdTemplateConfig,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    render_template_inner(template, ctx, cfg, rtss, delta_style, false)
}

fn render_template_inner(
    template: &str,
    ctx: &SectorLineCtx,
    cfg: &OsdTemplateConfig,
    rtss: bool,
    delta_style: &DeltaColorStyle,
    in_sub_slot: bool,
) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = template[i + 1..].find('}') {
                let key = &template[i + 1..i + 1 + end];
                let (name, spec) = key.split_once(':').unwrap_or((key, ""));
                let rep = match name {
                    "sector" => (ctx.sector_index + 1).to_string(),
                    "cum_delta" => format_number(ctx.cum_delta_sec, spec),
                    "cum_delta_colored" => {
                        let t = format_number(ctx.cum_delta_sec, if spec.is_empty() { "+.3" } else { spec });
                        if rtss {
                            delta_style.wrap_delta(ctx.cum_delta_sec, &t)
                        } else {
                            t
                        }
                    }
                    "tot" => format_number(ctx.tot_sec, if spec.is_empty() { "time" } else { spec }),
                    "ref" => ctx
                        .reference_tot_sec
                        .filter(|t| t.is_finite() && *t >= 0.0)
                        .map(|t| format_number(t, if spec.is_empty() { "time" } else { spec }))
                        .unwrap_or_else(|| "--:--.--".into()),
                    "time" => ctx
                        .subs
                        .first()
                        .and_then(|s| s.time_sec)
                        .map(|t| format_number(t, spec))
                        .unwrap_or_else(|| "--".into()),
                    "delta" => ctx
                        .subs
                        .first()
                        .and_then(|s| s.delta_sec)
                        .map(|d| format_number(d, if spec.is_empty() { "+.3" } else { spec }))
                        .unwrap_or_else(|| "--".into()),
                    "delta_colored" => ctx
                        .subs
                        .first()
                        .and_then(|s| s.delta_sec)
                        .map(|d| {
                            let t = format_number(d, if spec.is_empty() { "+.3" } else { spec });
                            if rtss {
                                delta_style.wrap_delta(d, &t)
                            } else {
                                t
                            }
                        })
                        .unwrap_or_else(|| "--".into()),
                    "subs" if !in_sub_slot => {
                        expand_subs(&cfg.sub_slot, ctx, cfg, rtss, delta_style)
                    }
                    "incomplete" => {
                        if ctx.incomplete {
                            "~".into()
                        } else {
                            String::new()
                        }
                    }
                    _ => format!("{{{key}}}"),
                };
                out.push_str(&rep);
                i += 1 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

pub fn format_sector_line_templated(
    cfg: &OsdTemplateConfig,
    ctx: &SectorLineCtx,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    format_line_templated(&cfg.sector_line, ctx, cfg, rtss, delta_style)
}

pub fn format_live_sector_line_templated(
    cfg: &OsdTemplateConfig,
    ctx: &SectorLineCtx,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    format_line_templated(&cfg.live_sector_line, ctx, cfg, rtss, delta_style)
}

fn format_line_templated(
    template: &str,
    ctx: &SectorLineCtx,
    cfg: &OsdTemplateConfig,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let mut line = render_template(template, ctx, cfg, rtss, delta_style);
    if ctx.incomplete && !template.contains("{incomplete}") {
        line = format!("{line}~");
    }
    line
}

pub fn format_finish_line_templated(
    cfg: &OsdTemplateConfig,
    ctx: &FinishLineCtx,
    rtss: bool,
    delta_style: &DeltaColorStyle,
) -> String {
    let mut out = String::new();
    let bytes = cfg.finish_line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = cfg.finish_line[i + 1..].find('}') {
                let key = &cfg.finish_line[i + 1..i + 1 + end];
                let (name, spec) = key.split_once(':').unwrap_or((key, ""));
                let rep = match name {
                    "cum_tot" => format_number(ctx.cum_tot_sec, if spec.is_empty() { "time" } else { spec }),
                    "ref_tot" => format_number(ctx.ref_tot_sec, if spec.is_empty() { "time" } else { spec }),
                    "delta" => format_number(ctx.cum_delta_sec, if spec.is_empty() { "+.3" } else { spec }),
                    "delta_colored" => {
                        let t = format_number(
                            ctx.cum_delta_sec,
                            if spec.is_empty() { "+.3" } else { spec },
                        );
                        if rtss {
                            delta_style.wrap_delta(ctx.cum_delta_sec, &t)
                        } else {
                            t
                        }
                    }
                    _ => format!("{{{key}}}"),
                };
                out.push_str(&rep);
                i += 1 + end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_template_reproduces_legacy_shape() {
        let cfg = OsdTemplateConfig::default();
        let ctx = SectorLineCtx {
            sector_index: 0,
            cum_delta_sec: 0.5,
            tot_sec: 35.5,
            reference_tot_sec: Some(91.45),
            incomplete: false,
            subs: vec![
                SubSlotCtx {
                    time_sec: Some(21.5),
                    delta_sec: Some(0.5),
                },
                SubSlotCtx {
                    time_sec: None,
                    delta_sec: None,
                },
            ],
        };
        let line = format_sector_line_templated(&cfg, &ctx, false, &DeltaColorStyle::default());
        assert!(line.contains("S1:"));
        assert!(line.contains("+0.500"));
        assert!(line.contains("[0:21.50]"));
        assert!(line.contains("[--]"));
        assert!(line.contains("ref:"));
        assert!(line.contains("tot:"));
    }

    #[test]
    fn minimal_live_is_delta_only() {
        let cfg = OsdTemplateConfigFile {
            preset: "minimal".into(),
            ..Default::default()
        }
        .to_runtime();
        assert_eq!(cfg.live_sector_line, "{cum_delta_colored}");
        let ctx = SectorLineCtx {
            sector_index: 2,
            cum_delta_sec: 1.25,
            tot_sec: 40.0,
            reference_tot_sec: Some(39.0),
            incomplete: false,
            subs: vec![],
        };
        let line = format_live_sector_line_templated(&cfg, &ctx, true, &DeltaColorStyle::default());
        assert!(!line.contains('S'));
        assert!(!line.contains("tot"));
        assert!(line.contains("<C="));
        assert!(line.contains("1.250") || line.contains("+1.25"));
    }

    #[test]
    fn wrap_rtss_font_scale_uses_s_reset_not_s_equals() {
        let s = wrap_rtss_font_scale("<C=00ff00>+0.12<C>", 150, true);
        assert!(s.starts_with("<S=150>"));
        assert!(s.ends_with("<S>"));
        assert!(!s.contains("<S=>"));
    }

    #[test]
    fn rtss_markup_passthrough() {
        let cfg = OsdTemplateConfig {
            sector_line: "<P4><L0>S{sector}: {cum_delta_colored}".into(),
            live_delta_font_scale: 0,
            ..Default::default()
        };
        let ctx = SectorLineCtx {
            sector_index: 1,
            cum_delta_sec: -0.2,
            tot_sec: 10.0,
            reference_tot_sec: None,
            incomplete: false,
            subs: vec![],
        };
        let line = format_sector_line_templated(&cfg, &ctx, true, &DeltaColorStyle::default());
        assert!(line.starts_with("<P4><L0>S2:"));
        assert!(line.contains("<C="));
    }
}
