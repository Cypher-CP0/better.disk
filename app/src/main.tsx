import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { createSource } from "./data/ipc";
import type { StartupReport } from "./data/model";
import "./styles.css";

const source = createSource();

// First paint: two animation frames after the first commit (App's mount
// effect). The native side relates this to OS process creation; the
// specification's target is launch to usable root picker within 3 s.
let resolveStartup: (r: StartupReport) => void = () => {};
let rejectStartup: (e: unknown) => void = () => {};
const startup = new Promise<StartupReport>((resolve, reject) => {
  resolveStartup = resolve;
  rejectStartup = reject;
});
let reported = false;
function onMounted() {
  if (reported) return; // StrictMode runs mount effects twice in development.
  reported = true;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      source.reportFirstPaint(performance.now()).then(resolveStartup, rejectStartup);
    }),
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");
createRoot(root).render(
  <StrictMode>
    <App source={source} startup={startup} onMounted={onMounted} />
  </StrictMode>,
);
