// Actual rendered-site checks using the runner's Chrome and Node built-ins only.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, readFile, writeFile, mkdir, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join, extname, sep } from 'node:path';

const site = resolve(process.argv[2] ?? 'docs/_site');
const evidence = resolve('validation-evidence/site-themes');
await mkdir(evidence, { recursive: true });
const sleep = ms => new Promise(done => setTimeout(done, ms));
const server = createServer(async (req, res) => {
  try {
    const name = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const path = resolve(site, '.' + name + (name.endsWith('/') ? 'index.html' : ''));
    assert(path.startsWith(site + sep));
    const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' };
    res.setHeader('Content-Type', types[extname(path)] ?? 'application/octet-stream');
    res.end(await readFile(path));
  } catch { res.statusCode = 404; res.end('Not found'); }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const base = `http://127.0.0.1:${server.address().port}/`;
const reports = [];

async function browser(systemTheme) {
  const profile = await mkdtemp(join(tmpdir(), 'proj-site-theme-'));
  const child = spawn(process.env.CHROME_BIN ?? 'google-chrome', [
    '--headless', '--disable-gpu', '--remote-debugging-port=0',
    '--window-size=1400,1800', '--user-data-dir=' + profile, 'about:blank',
  ], { stdio: ['ignore', 'ignore', 'pipe'] });
  let errors = '';
  child.stderr.on('data', data => { errors += data; });
  const exited = new Promise(done => child.once('exit', done));
  let socket;
  try {
    let port;
    for (let n = 0; n < 160; n++) {
      try { port = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; break; }
      catch { await sleep(100); }
    }
    assert(port, 'Chrome did not start: ' + errors.slice(-2000));
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    socket = new WebSocket(targets.find(t => t.type === 'page').webSocketDebuggerUrl);
    await new Promise((done, fail) => { socket.addEventListener('open', done, { once: true }); socket.addEventListener('error', fail, { once: true }); });
    let id = 0;
    const pending = new Map();
    socket.addEventListener('message', event => {
      const data = JSON.parse(event.data);
      if (!data.id) return;
      const request = pending.get(data.id);
      pending.delete(data.id);
      if (data.error) request.reject(new Error(JSON.stringify(data.error)));
      else request.resolve(data.result);
    });
    function call(method, params = {}) {
      return new Promise((resolve, reject) => { const next = ++id; pending.set(next, { resolve, reject }); socket.send(JSON.stringify({ id: next, method, params })); });
    }
    async function evaluate(expression) {
      const value = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      assert(!value.exceptionDetails, JSON.stringify(value.exceptionDetails));
      return value.result.value;
    }
    async function wait(expression) {
      for (let n = 0; n < 100; n++) { if (await evaluate(expression)) return; await sleep(50); }
      throw new Error('Timed out: ' + expression);
    }
    await call('Page.enable');
    await call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: systemTheme }] });
    async function navigate(page, theme) {
      await call('Page.navigate', { url: base + page + '.html' });
      await wait(`document.readyState === 'complete' && document.body.classList.contains('quarto-${theme}')`);
      await sleep(200);
    }
    const pages = (await readdir(site, { recursive: true })).filter(path => path.endsWith('.html')).map(path => path.slice(0, -5)).sort();
    for (const page of pages) {
      await navigate(page, systemTheme);
      const result = await evaluate(`(() => {
        const main = document.querySelector('main');
        const toggle = document.querySelector('.quarto-color-scheme-toggle');
        const rgba = value => value.match(/[\\d.]+/g).map(Number);
        const bg = el => { for (; el; el = el.parentElement) { const c = rgba(getComputedStyle(el).backgroundColor); if (c.length === 3 || c[3] > 0.99) return c; } return [255,255,255]; };
        const lum = c => c.slice(0,3).map(x => { x /= 255; return x <= .04045 ? x/12.92 : ((x+.055)/1.055)**2.4; }).reduce((s,x,i) => s+x*[.2126,.7152,.0722][i],0);
        const contrast = el => { const a=lum(rgba(getComputedStyle(el).color)), b=lum(bg(el)); return (Math.max(a,b)+.05)/(Math.min(a,b)+.05); };
        // SVG labels sit above painted shapes, not CSS backgrounds; review diagram screenshots separately.
        const samples = [...main.querySelectorAll('p,li,h1,h2,h3,a,code,code span,.btn,button,td,th'), ...document.querySelectorAll('.navbar .nav-link,.sidebar-item-text')].filter(el => el.innerText.trim() && el.getBoundingClientRect().height > 0 && !el.closest('svg'));
        const low = samples.map(el => ({ tag: el.tagName, text: el.innerText.slice(0,65), ratio: contrast(el) })).filter(x => x.ratio < 4.5);
        const broken = [...main.querySelectorAll('p')].filter(el => /(?:^|\\n)\\s*(?:[-*+]|\\d+[.)])\\s+/.test(el.innerText)).map(el=>el.innerText.slice(0,150));
        return { title: document.title, theme: document.body.className, lists: main.querySelectorAll('li').length, code: main.querySelectorAll('pre').length, tables: main.querySelectorAll('table').length,
          toggle: toggle && { role: toggle.getAttribute('role'), label: toggle.getAttribute('aria-label'), tab: toggle.tabIndex }, low, broken,
          prefixes: [...main.querySelectorAll('li')].map(el => el.innerText.trim()).filter(s => ['cache','raw','output','docs'].includes(s)) };
      })()`);
      reports.push({ page, systemTheme, ...result });
      assert.deepEqual(result.broken, [], `${page}: unrendered list markup`);
      if (page === 'cas-github' || page === 'xet-storage') {
        const kind = page === 'cas-github' ? 'cas' : 'xet';
        assert(await evaluate(`document.querySelector('main').innerText.includes('.proj/remotes/<remote-name-hash>/${kind}-github/v1/')`), 'Complete literal metadata path must remain visible');
      }
      assert.deepEqual(result.low, [], `${page} ${systemTheme}: low contrast ${JSON.stringify(result.low)}`);
      assert(result.toggle?.role === 'button' && result.toggle.tab === 0 && result.toggle.label, `${page}: accessible toggle`);
      if (page === 'configuration') assert.equal(result.prefixes.length, 4, 'Each directory prefix must be its own rendered list item');
      if (page === 'lifecycle' && systemTheme === 'dark') {
        assert(await evaluate(`(() => { const figures = [...document.querySelectorAll('svg.flowchart')]; return figures.length === 13 && figures.every(svg => getComputedStyle(svg).backgroundColor === 'rgb(248, 250, 249)'); })()`), 'Fixed-palette diagrams need a visible light canvas in dark mode');
      }
      if (['index','configuration','xet-storage','r-reference','lifecycle'].includes(page)) {
        const shot = await call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
        await writeFile(join(evidence, `${page}-${systemTheme}.png`), Buffer.from(shot.data, 'base64'));
      }
    }
    await navigate('index', systemTheme);
    await evaluate(`document.querySelector('.quarto-color-scheme-toggle').focus()`);
    await call('Input.dispatchKeyEvent', { type: 'keyDown', key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
    await call('Input.dispatchKeyEvent', { type: 'keyUp', key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
    const chosen = systemTheme === 'dark' ? 'light' : 'dark';
    await wait(`document.body.classList.contains('quarto-${chosen}')`);
    const focus = await evaluate(`(() => {
      const el = document.activeElement, style = getComputedStyle(el);
      const rgb = s => s.match(/[\\d.]+/g).map(Number);
      let bg = el; while (bg.parentElement && getComputedStyle(bg).backgroundColor === 'rgba(0, 0, 0, 0)') bg = bg.parentElement;
      const l = s => rgb(s).slice(0,3).map(v => { v /= 255; return v <= .04045 ? v/12.92 : ((v+.055)/1.055)**2.4; }).reduce((a,v,i) => a+v*[.2126,.7152,.0722][i],0);
      const ratio = (a,b) => (Math.max(a,b)+.05)/(Math.min(a,b)+.05);
      const background = l(getComputedStyle(bg).backgroundColor);
      return { outline: style.outlineStyle, width: parseFloat(style.outlineWidth), contrast: ratio(l(style.outlineColor),background), controlForegroundContrast: ratio(l(style.color),background), icon: getComputedStyle(el.querySelector('.bi'), '::before').backgroundImage };
    })()`);
    assert(focus.outline !== 'none' && focus.width >= 2 && focus.contrast >= 3, JSON.stringify(focus));
    assert(focus.icon !== 'none', 'Theme icon image must be present');
    // The SVG background image is reviewed in screenshots; anchor color is not its pixel color.
    await navigate('r-frontend', chosen);
    await call('Page.reload');
    await wait(`document.readyState === 'complete' && document.body.classList.contains('quarto-${chosen}')`);
    await evaluate(`document.querySelector('.quarto-color-scheme-toggle').focus()`);
    await call('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await call('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await wait(`document.body.classList.contains('quarto-${systemTheme}')`);
    await sleep(200);
    assert(await evaluate(`document.body.classList.contains('quarto-${systemTheme}')`), 'Enter must toggle exactly once');
    await navigate('configuration', systemTheme);
    reports.push({ systemTheme, keyboard: ['Space','Enter'], focus, persistedAcrossNavigationAndReload: chosen, enterPersists: systemTheme });
    console.log(`PASS ${systemTheme} system default, ${pages.length} pages, Space/Enter, persisted navigation/reload`);
  } catch (error) {
    console.error(error); // Preserve the actual assertion if browser cleanup also fails.
    throw error;
  } finally {
    socket?.close();
    child.kill('SIGTERM');
    await Promise.race([exited, sleep(5000)]);
    if (child.exitCode === null) { child.kill('SIGKILL'); await exited; }
    await writeFile(join(evidence, `browser-${systemTheme}.txt`), errors);
    await rm(profile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }).catch(error => {
      console.error('Chrome profile cleanup incomplete:', error.message);
    });
  }
}
try { await browser('light'); await browser('dark'); }
finally {
  await writeFile(join(evidence, 'report.json'), JSON.stringify(reports, null, 2));
  await new Promise(done => server.close(done));
}
