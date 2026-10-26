// Synthetic profile redaction and the explicit file-name sharing preview.
export function sanitize(text) {
  return text
    .replace(/C:\\Users\\Alex/gi, "%USERPROFILE%")
    .replace(/\bAlex\b/g, "[user]")
    .replace(/DESKTOP-ALEX/gi, "[computer]");
}

export function fileNameSharing(root) {
  if (!root) return { names: [], scope: null };
  return {
    scope: "%SCOPE%",
    names: [
      { item_ref: "scan-1:1", name: "Games", relative_path: "%SCOPE%\\Games" },
      { item_ref: "scan-1:2", name: "Videos", relative_path: "%SCOPE%\\Videos" },
      { item_ref: "scan-1:3", name: "windows-11-24h2.iso", relative_path: "%SCOPE%\\Installers\\windows-11-24h2.iso" },
    ],
  };
}
