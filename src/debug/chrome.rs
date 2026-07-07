use std::sync::Arc;

use gpui::Div;
use gpui::FontFeatures;
use gpui::ParentElement;
use gpui::SharedString;
use gpui::Styled;
use gpui::div;
use gpui::prelude::FluentBuilder;
use gpui::px;
use gpui::rgb;

#[derive(Clone, Copy, Debug)]
pub(in crate::debug) enum StatBlockWidth {
    Fixed(f32),
    Flexible,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::debug) struct StatBlockOptions {
    width: StatBlockWidth,
    unit: Option<&'static str>,
    value_color: u32,
    border_color: u32,
    tabular: bool,
    truncate: bool,
}

impl StatBlockOptions {
    pub(in crate::debug) const fn fixed(width: f32) -> Self {
        Self {
            width: StatBlockWidth::Fixed(width),
            unit: None,
            value_color: 0xf9fafb,
            border_color: 0x1f2937,
            tabular: false,
            truncate: false,
        }
    }

    pub(in crate::debug) const fn flexible() -> Self {
        Self {
            width: StatBlockWidth::Flexible,
            unit: None,
            value_color: 0xf9fafb,
            border_color: 0x1f2937,
            tabular: false,
            truncate: false,
        }
    }

    pub(in crate::debug) const fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }

    pub(in crate::debug) const fn value_color(mut self, value_color: u32) -> Self {
        self.value_color = value_color;
        self
    }

    pub(in crate::debug) const fn border_color(mut self, border_color: u32) -> Self {
        self.border_color = border_color;
        self
    }

    pub(in crate::debug) const fn tabular(mut self) -> Self {
        self.tabular = true;
        self
    }

    pub(in crate::debug) const fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
}

pub(in crate::debug) fn stats_row() -> Div {
    div()
        .rounded_md()
        .border_1()
        .border_color(rgb(0x1f2937))
        .bg(rgb(0x111827))
        .p(px(10.0))
        .flex()
        .gap_2()
        .flex_wrap()
}

pub(in crate::debug) fn stat_block(
    label: &str,
    value: impl Into<SharedString>,
    options: StatBlockOptions,
) -> Div {
    let value = div()
        .min_w_0()
        .whitespace_nowrap()
        .text_sm()
        .text_color(rgb(options.value_color))
        .when(options.tabular, |this| {
            this.font_features(FontFeatures(Arc::new(vec![("tnum".to_string(), 1)])))
        })
        .when(options.truncate, |this| this.truncate())
        .child(value.into());

    div()
        .rounded_sm()
        .border_1()
        .border_color(rgb(options.border_color))
        .bg(rgb(0x0b1020))
        .px(px(10.0))
        .py(px(8.0))
        .flex()
        .flex_col()
        .gap_1()
        .when_some(
            match options.width {
                StatBlockWidth::Fixed(width) => Some(width),
                StatBlockWidth::Flexible => None,
            },
            |this, width| this.w(px(width)),
        )
        .when(matches!(options.width, StatBlockWidth::Flexible), |this| {
            this.flex_1().min_w_0()
        })
        .child(
            div()
                .text_xs()
                .whitespace_nowrap()
                .text_color(rgb(0x6b7280))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap_1()
                .child(value)
                .when_some(options.unit, |this, unit| {
                    this.child(div().text_xs().text_color(rgb(0x6b7280)).child(unit))
                }),
        )
}
