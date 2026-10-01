// jsdom lays nothing out, so it never implemented `scrollIntoView`. The rail
// calls it on every selection move, and without the method the call throws
// inside a requestAnimationFrame: the assertions still pass, but every test
// that moves the selection leaves an uncaught exception behind and the run
// fails. There is nothing to scroll to in a document with no layout, so a
// no-op is what a browser-less environment should be doing anyway.
if (typeof Element !== "undefined" && !("scrollIntoView" in Element.prototype)) {
  Object.defineProperty(Element.prototype, "scrollIntoView", {
    value: () => undefined,
    writable: true,
    configurable: true,
  });
}
