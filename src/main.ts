import "./styles.css";
import { mountApp } from "./app";

const root = document.querySelector<HTMLElement>("#app");

if (!root) {
  throw new Error("Orivo could not find its application root.");
}

mountApp(root);
