import "./styles.css";
import { mountApp } from "./app";
import { mountStore } from "./store";

const root = document.querySelector<HTMLElement>("#app");

if (!root) {
  throw new Error("Orivo could not find its application root.");
}

mountApp(root);

// The Store ("Découvrir") page reuses the library's top navigation bar and
// swaps only the content beneath it. Its content mounts inside the selector
// shell so the shared `.topbar` stays in place across Library ⇆ Store.
const selector = root.querySelector<HTMLElement>(".selector");
const selectorContent = root.querySelector<HTMLElement>("#selector-content");

const storeContent = document.createElement("div");
storeContent.id = "store-content-view";
storeContent.hidden = true;
selectorContent?.appendChild(storeContent);
mountStore(storeContent);

const syncNav = (storeActive: boolean): void => {
  root.querySelectorAll<HTMLElement>(".primary-nav .nav-link[data-view]").forEach((link) => {
    const active = storeActive ? link.dataset.view === "store" : link.dataset.view === "library";
    link.classList.toggle("is-active", active);
    if (active) {
      link.setAttribute("aria-current", "page");
    } else {
      link.removeAttribute("aria-current");
    }
  });
};

const showStore = (active: boolean): void => {
  selector?.classList.toggle("is-store-active", active);
  storeContent.hidden = !active;
  syncNav(active);
};

root.addEventListener("click", (event) => {
  const link = (event.target as HTMLElement).closest<HTMLElement>(".primary-nav .nav-link[data-view]");
  if (!link) {
    return;
  }
  showStore(link.dataset.view === "store");
});
