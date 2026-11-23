// The app's page: what an open dialog covers, as against the layers drawn over
// it (the offline cover, the sign-in gate, the toasts).
export const PAGE_LAYER_ATTR = "data-dux-page"

export function isInPage(target: Element): boolean {
  return target.closest(`[${PAGE_LAYER_ATTR}]`) !== null
}
