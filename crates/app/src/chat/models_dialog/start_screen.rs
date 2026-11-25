//! The first screen: whether the models are hosted on one's own devices or come from a provider,
//! and, out of the way, what is connected already. Also the screen with the providers.

use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use pi_config::ProviderInfo;
use pi_config::providers::{BEST_KNOWN, BUILT_IN, BuiltIn};
use pi_config::server;

use super::{ModelsDialog, Screen, card, count, label};
use crate::ui::{AppIcon, FadingText, QuietButton as _, light};

/// Where the back button of a screen with one server or provider goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Back {
    #[default]
    Start,
    Hosted,
    Providers,
}

impl ModelsDialog {
    pub(super) fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        self.address_error = None;
        cx.notify();
    }

    /// The provider in the file for the server at `base_url`.
    pub(super) fn provider_at(&self, base_url: &str) -> Option<&ProviderInfo> {
        let same = |url: &str| url.trim_end_matches('/').replace("127.0.0.1", "localhost");
        self.providers.iter().find(|provider| {
            provider
                .base_url
                .as_deref()
                .is_some_and(|url| same(url) == same(base_url))
        })
    }

    pub(super) fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let hosted = choice(
            "start:hosted",
            AppIcon::Server,
            "Locally hosted model",
            "Ollama, LM Studio or another server, on this device or another one",
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Hosted, cx)));
        let provider = choice(
            "start:provider",
            AppIcon::Key,
            "API provider model",
            "Anthropic, OpenAI, Google Gemini, OpenRouter and more, with an API key or a sign-in",
            cx,
        )
        .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Providers, cx)));
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().debug_selector(|| "start:hosted".into()).child(hosted))
                    .child(
                        div()
                            .debug_selector(|| "start:provider".into())
                            .child(provider),
                    ),
            )
            .children(self.render_connected(cx))
            .child(
                h_flex().child(div().flex_1()).child(
                    div().debug_selector(|| "edit-models-json".into()).child(
                        Button::new("edit-models-json")
                            .quiet(cx)
                            .small()
                            .icon(Icon::from(AppIcon::SquarePen).text_color(muted))
                            .label("Edit models.json")
                            .on_click(cx.listener(|this, _, _, cx| this.show(Screen::Json, cx))),
                    ),
                ),
            )
            .into_any_element()
    }

    /// The providers in the file, as a quiet list: they are not what the screen is about.
    fn render_connected(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.providers.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let rows = self.providers.iter().map(|provider| {
            let id = provider.id.clone();
            let (icon, detail) = describe(provider, &self.available);
            let title = provider.label().to_owned();
            let selector = format!("provider:{id}");
            div().debug_selector(|| selector.clone()).child(
                Button::new(SharedString::from(selector.clone()))
                    .quiet(cx)
                    .w_full()
                    .h(px(30.))
                    .accessibility_label(format!("{title}, {detail}"))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .text_sm()
                            .child(icon.xsmall().text_color(muted))
                            .child(div().min_w_0().child(FadingText::new(title)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(muted)
                                    .child(FadingText::new(detail)),
                            )
                            .child(Icon::new(IconName::ChevronRight).xsmall().text_color(muted)),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.back_to = Back::Start;
                        this.open_provider(&id, window, cx);
                    })),
            )
        });
        Some(
            v_flex()
                .gap_1()
                .pt_3()
                .border_t_1()
                .border_color(border)
                .child(label("Connected", cx))
                .child(
                    div()
                        .id("models-connected")
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .child(v_flex().children(rows)),
                ),
        )
    }

    /// pi's own providers.
    pub(super) fn render_providers(&self, cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<Div> = BUILT_IN[..BEST_KNOWN]
            .iter()
            .map(|provider| {
                let models = self.available.get(provider.id).copied().unwrap_or_default();
                let selector = format!("built-in:{}", provider.id);
                div()
                    .debug_selector(|| selector.clone())
                    .w(relative(0.49))
                    .child(
                        card(
                            selector.clone(),
                            AppIcon::Key,
                            provider.label,
                            SharedString::default(),
                            (models > 0).then(|| available(Some(models), cx)),
                            false,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.back_to = Back::Providers;
                                this.open_provider(provider.id, window, cx);
                            },
                        )),
                    )
            })
            .collect();
        let weak = cx.weak_entity();
        let more = Button::new("more-providers")
            .quiet(cx)
            .small()
            .label("More providers")
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| more_providers(menu, &weak));
        v_flex()
            .gap_3()
            .child(Self::render_top("API provider model", None, cx))
            .child(label(
                "With an API key, or a sign-in where the provider has one",
                cx,
            ))
            .child(grid(cards))
            .child(h_flex().child(more))
            .into_any_element()
    }
}

/// One of the two big choices of the first screen.
fn choice(
    id: &'static str,
    icon: impl Into<Icon>,
    title: &'static str,
    detail: &'static str,
    cx: &App,
) -> Button {
    let muted = cx.theme().muted_foreground;
    Button::new(id)
        .choice_card(false, cx)
        .h(px(62.))
        .accessibility_label(format!("{title}. {detail}"))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_3()
                .child(icon.into().text_color(muted))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(div().min_w_0().child(FadingText::new(title)))
                        .child(
                            div()
                                .min_w_0()
                                .text_xs()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(muted)
                                .child(FadingText::new(detail)),
                        ),
                )
                .child(Icon::new(IconName::ChevronRight).small().text_color(muted)),
        )
}

/// Cards, two in a row.
pub(super) fn grid(cards: Vec<Div>) -> Div {
    h_flex()
        .flex_wrap()
        .justify_between()
        .gap_y_2()
        .children(cards)
}

fn more_providers(
    menu: component::menu::PopupMenu,
    weak: &WeakEntity<ModelsDialog>,
) -> component::menu::PopupMenu {
    BUILT_IN[BEST_KNOWN..].iter().fold(
        menu.scrollable(true).max_h(px(300.)),
        |menu, provider: &'static BuiltIn| {
            let weak = weak.clone();
            menu.item(
                PopupMenuItem::new(provider.label).on_click(move |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.back_to = Back::Providers;
                        this.open_provider(provider.id, window, cx);
                    });
                }),
            )
        },
    )
}

/// A green light and how many models there are, or that a key is wanted first.
pub(super) fn available(models: Option<usize>, cx: &App) -> AnyElement {
    h_flex()
        .flex_none()
        .gap_1p5()
        .child(light(crate::theme::palette(cx).success))
        .child(label(
            match models {
                Some(models) => count(models, "model"),
                None => "Wants a key".to_owned(),
            },
            cx,
        ))
        .into_any_element()
}

/// What to show of a provider in the file: its icon, and where its models come from and how many
/// there are.
pub fn describe(
    provider: &ProviderInfo,
    available: &std::collections::HashMap<String, usize>,
) -> (Icon, String) {
    if provider.built_in().is_some() {
        let detail = match available.get(&provider.id) {
            Some(models) => format!("API key · {}", count(*models, "model")),
            None => "API key".to_owned(),
        };
        return (AppIcon::Key.into(), detail);
    }
    let models = count(provider.models.len(), "model");
    match &provider.base_url {
        Some(url) => (
            AppIcon::Server.into(),
            format!("{} · {models}", server::short_address(url)),
        ),
        None => (AppIcon::Key.into(), models),
    }
}
