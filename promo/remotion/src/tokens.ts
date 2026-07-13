// Brand tokens — pulled from claw-fleet-desktop/app/App.css (warm-paper light
// theme + dark terminal surfaces). Do not invent new colors here.
export const T = {
  paper: "#f1efea",
  paperDeep: "#e7e4dd",
  card: "#fbfaf7",
  ink: "#1f2023",
  inkSecondary: "#5d6168",
  inkDim: "#6f7078",
  border: "rgba(32, 28, 18, 0.11)",
  borderStrong: "rgba(32, 28, 18, 0.22)",
  coral: "#c25232",
  coralHover: "#a94527",
  coralSoft: "rgba(194, 82, 50, 0.13)",
  night: "#101113",
  nightDeep: "#08090a",
  nightText: "#f7f8f8",
  nightDim: "#8a8f98",
  coralNight: "#d97757",
  // status badges, straight from the app
  badge: {
    thinking: { bg: "#f3e8ff", fg: "#6d28d9" },
    executing: { bg: "#cffafe", fg: "#0e7490" },
    streaming: { bg: "#dcfce7", fg: "#15803d" },
    waiting: { bg: "#fef3c7", fg: "#b45309" },
    delegating: { bg: "#fce7f3", fg: "#be185d" },
  },
  green: "#15803d",
  red: "#b91c1c",
  amber: "#b45309",
};

export const FONT = {
  display: "'Fraunces', Georgia, serif",
  body: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif",
  mono: "'JetBrains Mono', 'Cascadia Code', Menlo, monospace",
};
