import "./styles.css";
import { mountApp } from "./app";
import { createBootSplash, revealBootSplash } from "./boot-splash";
import { applyFormFactor } from "./form-factor";
import { applyRememberedMotionPreference } from "./motion";
import { currentLanguage, setLanguage } from "./i18n";

const root = document.querySelector<HTMLElement>("#app");

if (!root) {
  throw new Error("Orivo could not find its application root.");
}

// Before the first paint: the attribute decides the layout every sheet below
// reads, so setting it after mounting would flash the desktop scene.
applyFormFactor();
// Likewise for motion: a "Reduced" chosen last time has to hold for the
// startup animation revealed just below, long before the host has been asked.
applyRememberedMotionPreference();
// The language was read from local memory at import; tell <html> for screen
// readers, hyphenation and the system spellchecker.
setLanguage(currentLanguage());

// The startup animation: shown when this is an app starting, dropped when it is
// a preview reloading. Started before mounting, so its cap counts from the
// start of the page and not from whenever the shell finished building.
revealBootSplash();
const bootSplash = createBootSplash();

mountApp(root, { bootSplash });
