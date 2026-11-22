import { onTestFinished } from "vitest"

// jsdom has no Web Animations, so a Base UI popup unmounts the moment it
// closes. This makes every element report one animation that runs until the
// test ends, so a closing modal stays on screen as it does in a browser for
// the length of its close animation. Called inside a test; undone when it ends.
export function holdCloseAnimations(): void {
  const had = Object.prototype.hasOwnProperty.call(Element.prototype, "getAnimations")
  const before = Element.prototype.getAnimations
  const running = { finished: new Promise<never>(() => {}) } as unknown as Animation
  Element.prototype.getAnimations = () => [running]
  onTestFinished(() => {
    if (had) Element.prototype.getAnimations = before
    else delete (Element.prototype as Partial<Element>).getAnimations
  })
}
