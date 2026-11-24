//! Icons bundled by the app (gpui-kit ships a subset of Lucide).

use gpui_kit::component::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppIcon {
    Brain,
    CodeXml,
    Files,
    GitBranch,
    GitCompare,
    Globe,
    Image,
    Library,
    MessageSquare,
    Package,
    Paperclip,
    Pencil,
    PiLogo,
    RotateCcw,
    Sparkles,
    SquarePen,
    Trash,
    Wrench,
}

impl AppIcon {
    pub fn path(self) -> &'static str {
        match self {
            Self::Brain => "icons/brain.svg",
            Self::CodeXml => "icons/code-xml.svg",
            Self::Files => "icons/files.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::GitCompare => "icons/git-compare.svg",
            Self::Globe => "icons/globe.svg",
            Self::Image => "icons/image.svg",
            Self::Library => "icons/library.svg",
            Self::MessageSquare => "icons/message-square.svg",
            Self::Package => "icons/package.svg",
            Self::Paperclip => "icons/paperclip.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::PiLogo => "icons/pi-logo.svg",
            Self::RotateCcw => "icons/rotate-ccw.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::SquarePen => "icons/square-pen.svg",
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
