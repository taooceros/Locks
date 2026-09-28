"use strict";

(() => {
  const byId = (id) => document.getElementById(id);
  const format = (value) => value.toLocaleString("en", { maximumFractionDigits: 2 });

  function serviceExample() {
    let credits = [0, 0];
    let counts = [0, 0];
    let history = [];
    const policy = () => document.querySelector('input[name="service-policy"]:checked').value;
    const longCost = () => Number(byId("b-service").value);

    function render() {
      const largest = Math.max(...credits, 1);
      ["a", "b"].forEach((client, index) => {
        byId(`${client}-total`).textContent = `${credits[index]} units · ${counts[index]} operations`;
        byId(`${client}-credit-bar`).style.width = `${100 * credits[index] / largest}%`;
      });
      byId("b-service-value").value = longCost();
      const sequence = byId("service-history");
      sequence.replaceChildren();
      history.forEach((client) => {
        const chip = document.createElement("span");
        chip.className = `client-${client.toLowerCase()}`;
        chip.textContent = client;
        sequence.append(chip);
      });
      if (!history.length) sequence.textContent = "No operations yet.";
      const spread = Math.abs(credits[0] - credits[1]);
      const explanation = policy() === "minimum"
        ? `The ideal minimum-credit bound here is ${longCost()} units; the spread stays within it.`
        : "Equal turns balance counts, not service when operation costs differ.";
      byId("service-status").textContent = `Step ${history.length} of 16. Service spread: ${spread} units. ${explanation}`;
      byId("service-next").disabled = history.length >= 16;
    }

    function reset() {
      credits = [0, 0];
      counts = [0, 0];
      history = [];
      render();
    }

    byId("service-next").addEventListener("click", () => {
      if (history.length >= 16) return;
      const selected = policy() === "turns" ? history.length % 2 : (credits[0] <= credits[1] ? 0 : 1);
      credits[selected] += selected === 0 ? 1 : longCost();
      counts[selected] += 1;
      history.push(selected === 0 ? "A" : "B");
      render();
    });
    byId("service-reset").addEventListener("click", reset);
    byId("b-service").addEventListener("input", reset);
    document.querySelectorAll('input[name="service-policy"]').forEach((input) => input.addEventListener("change", reset));
    reset();
    byId("service-lab").hidden = false;
  }

  function executionExample() {
    let completed = 0;
    const clients = ["A", "B", "A", "B"];
    const mode = () => document.querySelector('input[name="execution-mode"]:checked').value;

    function render() {
      const delegated = mode() === "combiner";
      const client = clients[completed - 1];
      const core = completed ? (delegated || client === "A" ? 0 : 1) : -1;
      byId("core-0-label").textContent = delegated ? "Combiner E / client A" : "Client A";
      [0, 1].forEach((index) => {
        byId(`core-${index}`).classList.toggle("active", index === core);
        byId(`core-${index}-work`).textContent = index === core
          ? `Just executed ${client}’s request`
          : (completed ? "Did not execute this step" : "Waiting to start");
      });
      [...byId("execution-sequence").children].forEach((chip, index) => {
        chip.dataset.done = String(index < completed);
        chip.removeAttribute("aria-current");
        if (index === completed - 1) chip.setAttribute("aria-current", "step");
      });
      const changes = delegated ? 0 : Math.max(0, completed - 1);
      const outcome = completed
        ? `Client ${client} received service; core ${core} executed it.`
        : "No request has run yet.";
      byId("execution-status").textContent = `Step ${completed} of 4. ${outcome} Executor changes: ${changes}. ${delegated ? "Requests and results still communicate across cores; no cache-residency guarantee follows." : "Changing executor can require protected-state movement; the traffic is not simulated here."}`;
      byId("execution-next").disabled = completed === clients.length;
    }

    function reset() { completed = 0; render(); }
    byId("execution-next").addEventListener("click", () => {
      if (completed < clients.length) completed += 1;
      render();
    });
    byId("execution-reset").addEventListener("click", reset);
    document.querySelectorAll('input[name="execution-mode"]').forEach((input) => input.addEventListener("change", reset));
    reset();
    byId("execution-lab").hidden = false;
  }

  function costExample() {
    const keys = ["h", "m", "d", "a", "b"];
    const presets = {
      reuse: { h: 2, m: 10, d: 4, a: 24, b: 6 },
      sparse: { h: 2, m: 10, d: 4, a: 24, b: 0.5 },
      local: { h: 2, m: 1, d: 4, a: 24, b: 6 },
      requests: { h: 2, m: 10, d: 14, a: 24, b: 6 },
    };
    const svgNS = "http://www.w3.org/2000/svg";
    function element(tag, attributes = {}, text) {
      const node = document.createElementNS(svgNS, tag);
      for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
      if (text !== undefined) node.textContent = text;
      return node;
    }

    function drawChart({ h, m, d, a, b }) {
      const svg = byId("cost-chart");
      const ceiling = Math.max(h + m, d + a, d + a / b, 1) * 1.2;
      const left = 65, right = 720, top = 48, bottom = 255;
      const x = (value) => left + (value - 0.25) / 15.75 * (right - left);
      const y = (value) => bottom - value / ceiling * (bottom - top);
      svg.replaceChildren(
        element("title", { id: "cost-chart-title" }, "Modeled overhead versus mean completions per pass"),
        element("desc", { id: "cost-chart-desc" }, `Toy parameters: CFL overhead is ${format(h + m)}. FC-PQ overhead at occupancy ${format(b)} is ${format(d + a / b)} and approaches floor ${format(d)}. Curve portions above the vertical range are clipped. These are not measurements.`),
      );
      const defs = element("defs");
      const clip = element("clipPath", { id: "cost-plot-clip" });
      clip.append(element("rect", { x: left, y: top, width: right - left, height: bottom - top }));
      defs.append(clip);
      svg.append(defs);
      const group = element("g", { "font-family": "system-ui, sans-serif", "font-size": 17, fill: "#18323d" });
      [0, ceiling / 2, ceiling].forEach((value) => {
        group.append(element("line", { x1: left, y1: y(value), x2: right, y2: y(value), stroke: "#d5dfe3" }));
        group.append(element("text", { x: left - 10, y: y(value) + 6, "text-anchor": "end" }, format(value)));
      });
      [0.25, 4, 8, 12, 16].forEach((value) => {
        group.append(element("text", { x: x(value), y: bottom + 27, "text-anchor": "middle" }, format(value)));
      });
      group.append(element("text", { x: left, y: 23, fill: "#9a4f24" }, "CFL: h + m (dashed)"));
      group.append(element("text", { x: 380, y: 23, fill: "#7057a3" }, "FC-PQ: d + a/b (solid)"));
      group.append(element("text", { x: 393, y: 320, "text-anchor": "middle" }, "Mean completions per pass · b"));
      const plot = element("g", { "clip-path": "url(#cost-plot-clip)" });
      plot.append(element("line", { x1: left, y1: y(h + m), x2: right, y2: y(h + m), stroke: "#9a4f24", "stroke-width": 3, "stroke-dasharray": "9 5" }));
      plot.append(element("line", { x1: left, y1: y(d), x2: right, y2: y(d), stroke: "#526575", "stroke-width": 2, "stroke-dasharray": "2 5" }));
      const points = [];
      for (let index = 0; index <= 240; index += 1) {
        const occupancy = 0.25 + index / 240 * 15.75;
        points.push(`${x(occupancy)},${y(d + a / occupancy)}`);
      }
      plot.append(element("polyline", { points: points.join(" "), fill: "none", stroke: "#7057a3", "stroke-width": 3 }));
      plot.append(element("line", { x1: x(b), y1: top, x2: x(b), y2: bottom, stroke: "#607f99", "stroke-width": 1.5, "stroke-dasharray": "4 4" }));
      plot.append(element("circle", { cx: x(b), cy: y(d + a / b), r: 5, fill: "#7057a3", stroke: "white", "stroke-width": 1.5 }));
      group.append(plot);
      svg.append(group);
    }

    function render() {
      const values = Object.fromEntries(keys.map((key) => [key, Number(byId(`cost-${key}`).value)]));
      const { h, m, d, a, b } = values;
      keys.forEach((key) => { byId(`cost-${key}-value`).value = format(values[key]); });
      const cfl = h + m;
      const pq = d + a / b;
      const scale = Math.max(cfl, pq, 1);
      for (const [id, value] of [["bar-h", h], ["bar-m", m], ["bar-d", d], ["bar-pass", a / b]]) {
        byId(id).style.width = `${100 * value / scale}%`;
      }
      byId("cost-cfl-total").value = format(cfl);
      byId("cost-pq-total").value = format(pq);
      const difference = cfl - pq;
      const tie = Math.abs(difference) < 1e-9;
      const result = tie ? "Tie in modeled overhead." : difference > 0
        ? `FC-PQ has ${format(difference)} fewer overhead units per completion in this model.`
        : `FC-PQ has ${format(-difference)} more overhead units per completion in this model.`;
      byId("cost-status").textContent = `CFL: ${format(cfl)}. FC-PQ: ${format(pq)}. ${result} This is not a measured speedup.`;
      byId("cost-status").dataset.outcome = tie ? "tie" : difference > 0 ? "win" : "lose";
      const gap = cfl - d;
      byId("cost-threshold").textContent = gap <= 0
        ? `No positive batch occupancy yields a strict win: the per-request floor d = ${format(d)} is already at least h + m = ${format(cfl)}.`
        : a === 0
          ? "The pass cost is zero and the per-request floor is lower: every positive occupancy gives lower modeled overhead."
          : `Strict advantage requires b > ${format(a / gap)} (threshold rounded for display). The floor d = ${format(d)} remains as b grows. The dotted line marks that floor; curve portions above the vertical range are clipped.`;
      drawChart(values);
    }
    keys.forEach((key) => byId(`cost-${key}`).addEventListener("input", render));
    document.querySelectorAll("[data-preset]").forEach((button) => {
      button.addEventListener("click", () => {
        for (const [key, value] of Object.entries(presets[button.dataset.preset])) byId(`cost-${key}`).value = value;
        render();
      });
    });
    render();
    byId("cost-lab").hidden = false;
    document.querySelectorAll(".static-only").forEach((node) => { node.hidden = true; });
  }

  serviceExample();
  executionExample();
  costExample();

  let printDetails = [];
  window.addEventListener("beforeprint", () => {
    printDetails = [...document.querySelectorAll("details")].map((node) => [node, node.open]);
    printDetails.forEach(([node]) => { node.open = true; });
  });
  window.addEventListener("afterprint", () => {
    printDetails.forEach(([node, open]) => { node.open = open; });
  });
})();
