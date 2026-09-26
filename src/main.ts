import "./styles.css";
import { mountApp } from "./app";
import { applyFormFactor } from "./form-factor";

const root = document.querySelector<HTMLElement>("#app");

if (!root) {
  throw new Error("Orivo could not find its application root.");
}

// Before the first paint: the attribute decides the layout every sheet below
// reads, so setting it after mounting would flash the desktop scene.
applyFormFactor();

mountApp(root);
