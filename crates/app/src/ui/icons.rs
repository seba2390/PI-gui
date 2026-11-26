//! Icons bundled by the app (gpui-kit ships a subset of Lucide).

use gpui_kit::component::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppIcon {
    Brain,
    CodeXml,
    Download,
    Files,
    GitBranch,
    GitCompare,
    Globe,
    Image,
    Key,
    Library,
    Link,
    MessageSquare,
    Package,
    Paperclip,
    Pencil,
    PiLogo,
    Play,
    RotateCcw,
    Server,
    ShieldCheck,
    ShieldOff,
    Sparkles,
    SquarePen,
    TextWrap,
    Trash,
    Wrench,
}

impl AppIcon {
    pub fn path(self) -> &'static str {
        match self {
            Self::Brain => "icons/brain.svg",
            Self::CodeXml => "icons/code-xml.svg",
            Self::Download => "icons/download.svg",
            Self::Files => "icons/files.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::GitCompare => "icons/git-compare.svg",
            Self::Globe => "icons/globe.svg",
            Self::Image => "icons/image.svg",
            Self::Key => "icons/key.svg",
            Self::Library => "icons/library.svg",
            Self::Link => "icons/link.svg",
            Self::MessageSquare => "icons/message-square.svg",
            Self::Package => "icons/package.svg",
            Self::Paperclip => "icons/paperclip.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::PiLogo => "icons/pi-logo.svg",
            Self::Play => "icons/play.svg",
            Self::RotateCcw => "icons/rotate-ccw.svg",
            Self::Server => "icons/server.svg",
            Self::ShieldCheck => "icons/shield-check.svg",
            Self::ShieldOff => "icons/shield-off.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::SquarePen => "icons/square-pen.svg",
            Self::TextWrap => "icons/text-wrap.svg",
            Self::Trash => "icons/trash.svg",
            Self::Wrench => "icons/wrench.svg",
        }
    }
}

impl From<AppIcon> for Icon {
    fn from(icon: AppIcon) -> Self {
        Icon::default().path(icon.path())
    }
}
