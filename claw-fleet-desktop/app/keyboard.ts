/** Native buttons activate with Enter or Space. ARIA button-like elements must
 * mirror both keys instead of supporting Enter only. */
export function isKeyboardActivationKey(key: string): boolean {
  return key === "Enter" || key === " ";
}
