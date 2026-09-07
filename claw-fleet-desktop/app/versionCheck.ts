export function versionCheckArgs(force: boolean, language?: string) {
  return {
    force,
    locale: language || "en",
  };
}
