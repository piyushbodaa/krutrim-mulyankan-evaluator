"use strict";
/* At a glance: the results explained for people new to AI.
   Reads the same payload as the Compare board (task IDs, subjects, languages, PASS/FAIL and wait
   times; never prompts or answers) and shows it with everyday instruments: a report card with
   phone-signal bars, a speedometer, a download-style progress bar, chat bubbles and a phone-shop
   comparison table. Every mark maps to measured data: one bar, dot or tick is one task.
   Shared verbatim by preview/ and src/kritrim_mulyankan/ui_assets/; uses helpers from app.js. */

const glance = {payload: null, job: null, adapters: null, onlyDiff: false, chatTimers: [], chatFrame: 0, titled: false};

const GLANCE_SKILLS = {
  rupee_numeracy: {name: 'Money & numbers', short: 'money', icon: 'rupee'},
  india_scheduling: {name: 'Dates & schedules', short: 'dates', icon: 'calendar'},
  record_reconciliation: {name: 'Office records', short: 'office records', icon: 'files'},
  hindi_hinglish_instructions: {name: 'Hindi & Hinglish', short: 'Hindi and Hinglish', icon: 'speech'},
  evidence_and_freshness: {name: 'Checking facts', short: 'fact-checking', icon: 'search'},
  robustness_and_abstention: {name: 'Avoiding mistakes', short: 'avoiding mistakes', icon: 'shield'},
};
// The bubble text is an illustration of each language, not a question from the test.
const GLANCE_LANGUAGES = [
  {codes: ['en-IN', 'en'], name: 'English', native: 'English', sample: 'Is there a meeting tomorrow?'},
  {codes: ['hi'], name: 'Hindi', native: 'हिन्दी', lang: 'hi', sample: 'क्या कल मीटिंग है?'},
  {codes: ['hi-Latn'], name: 'Hinglish', native: 'Hinglish', sample: 'Kal meeting hai kya?'},
];
const GLANCE_KINDS = {
  harness: ['AI coding assistant', 'AI coding assistants', 'Runs on a computer'],
  model_api: ['AI model', 'AI models', 'Used through its API'],
  consumer_app: ['chat app', 'chat apps', 'Recorded by a person'],
  manual_app: ['chat app', 'chat apps', 'Recorded by a person'],
  mock_validation: ['test control', 'test controls', 'Checks the evaluator; not an AI'],
  mock: ['test control', 'test controls', 'Checks the evaluator; not an AI'],
};
const GLANCE_ICONS = {
  rupee: 'M7 4h10M7 8.5h10M9.5 4c3.5 0 5.5 1.6 5.5 4.5S13 13 9.5 13H7.5l7.5 7',
  calendar: 'M5 6h14v14H5zM5 10h14M9 4v4M15 4v4',
  files: 'M8 3h8l4 4v12H8zM16 3v4h4M5 7v14h11',
  speech: 'M4 5h16v11h-9l-5 4v-4H4zM8 9h8M8 12h5',
  search: 'M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13zM15.5 15.5 20 20',
  shield: 'M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6zM9 12l2 2 4-4',
  target: 'M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10zM12 11.2a.8.8 0 1 0 0 1.6.8.8 0 0 0 0-1.6z',
  gauge: 'M4 17a8 8 0 1 1 16 0M12 17l4-5',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  cross: 'M6.5 6.5l11 11M17.5 6.5l-11 11',
};
const GLANCE_WORDS = ['No', 'One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight'];

/* ---------- Small helpers ---------- */
function gSvg(tag, attributes = {}) {
  const element = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
  return element;
}
function gIcon(name) {
  const svg = gSvg('svg', {viewBox: '0 0 24 24', 'aria-hidden': 'true', class: 'g-icon'});
  svg.append(gSvg('path', {d: GLANCE_ICONS[name] || GLANCE_ICONS.target}));
  return svg;
}
function gList(items) {
  return items.length < 2 ? items.join('') : items.slice(0, -1).join(', ') + ' and ' + items[items.length - 1];
}
function gPlural(count, word) { return count + ' ' + word + (count === 1 ? '' : 's'); }
function gWait(value) { return typeof value === 'number' ? seconds(value, 1) : '—'; }
function gClock(total) {
  if (total < 1) return seconds(total, 1);
  if (total < 60) return (total < 10 ? total.toFixed(1) : Math.round(total)) + ' s';
  const rounded = Math.round(total), minutes = Math.floor(rounded / 60), rest = rounded % 60;
  if (minutes < 60) return minutes + ' min' + (rest ? ' ' + rest + ' s' : '');
  return Math.floor(minutes / 60) + ' h ' + (minutes % 60) + ' min';
}
function gAbout(value) {
  if (value < 1) return 'under a second';
  return value < 90 ? Math.round(value) + ' seconds' : gClock(value);
}
function gTimes(ratio) { return (ratio < 10 ? Math.round(ratio * 2) / 2 : Math.round(ratio)) + ' times'; }
function gBand(wait) {
  if (wait < 0.3) return {label: 'Instant', like: 'Faster than you can blink.'};
  if (wait < 1) return {label: 'Instant', like: 'Under a second.'};
  if (wait < 10) return {label: 'Quick', like: 'About the time it takes to unlock your phone and open an app.'};
  if (wait < 45) return {label: 'A short wait', like: 'About as long as a short red light at a traffic signal.'};
  if (wait < 120) return {label: 'A noticeable wait', like: 'About as long as a full red light at a busy junction.'};
  return {label: 'A long wait', like: 'Long enough to make a cup of chai.'};
}
function gTrip(total) {
  if (total < 60) return 'Under a minute.';
  if (total < 300) return 'About as long as a short phone call.';
  if (total < 1200) return 'About as long as making a cup of chai.';
  if (total < 3600) return 'About as long as a TV episode.';
  return 'Over an hour.';
}
function gRateText(rate) {
  if (!Number.isFinite(rate) || rate >= 1000) return 'Over a thousand answers a minute';
  if (rate >= 1.5) return 'About ' + Math.round(rate) + ' answers a minute';
  if (rate >= 0.75) return 'About one answer a minute';
  return 'About one answer every ' + gAbout(60 / rate);
}
function gTitle(id) { return state.tasks.find(task => task.id === id)?.title || id; }
function gSkill(key) {
  return GLANCE_SKILLS[key] || {name: label(key), short: label(key).toLowerCase(), icon: 'target'};
}
function gKind(track) { return GLANCE_KINDS[track] || ['AI system', 'AI systems', 'Evaluated system']; }
function gAvatar(system, small) {
  const badge = node('span', system.initials, 'g-avatar' + (system.index === 0 ? ' g-first' : '') + (small ? ' g-small' : ''));
  badge.setAttribute('aria-hidden', 'true');
  return badge;
}
function gWho(system, small) {
  const who = node('span', undefined, 'g-who');
  who.append(gAvatar(system, small), node('span', system.short));
  return who;
}
function gSection(kicker, title, text, id) {
  const section = node('section', undefined, 'g-section'), head = node('div', undefined, 'g-head'), heading = node('h2', title);
  heading.id = 'glance-' + id;
  section.setAttribute('aria-labelledby', heading.id);
  head.append(node('p', kicker, 'eyebrow'), heading);
  if (text) head.append(node('p', text));
  section.append(head);
  return section;
}
function gMeter(passed, total, text, className) {
  const meter = node('span', undefined, className);
  meter.setAttribute('role', 'img');
  meter.setAttribute('aria-label', text);
  return meter;
}
// Phone-signal bars: one bar per task, full when right on every try, half when right on some.
function gSignal(passed, total, text, sometimes = 0) {
  const signal = gMeter(passed, total, text, 'g-signal'), shown = Math.min(total, 12), scale = shown / Math.max(total, 1);
  const full = Math.round(passed * scale), half = Math.round((passed + sometimes) * scale) - full;
  for (let index = 0; index < shown; index++) {
    const bar = node('i', undefined, index < full ? 'on' : index < full + half ? 'half' : '');
    bar.style.height = Math.round(30 + 70 * (index + 1) / shown) + '%';
    signal.append(bar);
  }
  return signal;
}
function gDots(passed, total, text, sometimes = 0) {
  const dots = gMeter(passed, total, text, 'g-dots');
  for (let index = 0; index < total; index++) dots.append(node('i', undefined, index < passed ? 'on' : index < passed + sometimes ? 'half' : ''));
  return dots;
}
function gMarks(tally) {
  const marks = node('b', tally.passed + '/' + tally.total);
  if (tally.sometimes) marks.append(node('small', ' +' + tally.sometimes + ' sometimes'));
  return marks;
}
function gPercent(value) { return value === null || value === undefined ? '—' : Math.round(value * 100) + '%'; }
function gPair(data, a, b, metric) {
  return data.verdicts.find(v => v.metric === metric && ((v.runner_a === a && v.runner_b === b) || (v.runner_a === b && v.runner_b === a))) || null;
}

/* ---------- Data: one record per system, from the board's own reading of the payload ---------- */
function glanceData(payload, adapters) {
  const ids = [...new Set(payload.results.map(row => row.task_id))];
  const firstRow = id => payload.results.find(row => row.task_id === id);
  const categories = [...new Set(ids.map(id => firstRow(id).category || 'other'))];
  const present = [...new Set(payload.results.map(row => row.language).filter(Boolean))];
  const languages = GLANCE_LANGUAGES.filter(item => item.codes.some(code => present.includes(code)));
  for (const code of present) {
    if (!GLANCE_LANGUAGES.some(item => item.codes.includes(code))) languages.push({codes: [code], name: code, native: code, sample: null});
  }
  // Task-level counts: right on every try, right on some tries, out of the tasks this system ran.
  const tally = (system, taskIds) => {
    const stats = taskIds.map(id => system.tasks.get(id)).filter(Boolean);
    return {passed: stats.filter(stat => stat.key === 'pass').length, sometimes: stats.filter(stat => stat.key === 'mixed').length, total: stats.length};
  };
  const inSkill = key => ids.filter(id => (firstRow(id).category || 'other') === key);
  const inLanguage = item => ids.filter(id => item.codes.includes(firstRow(id).language));
  const systems = boardSystems(payload, adapters).map((system, index) => {
    const stats = [...system.tasks.values()], waits = stats.map(stat => stat.wait).filter(value => typeof value === 'number');
    return {
      ...system,
      index,
      initials: system.short.split(/[\s-]+/).filter(Boolean).map(word => word[0]).join('').slice(0, 2).toUpperCase() || '?',
      version: system.name.slice(system.short.length).trim(),
      kind: gKind(system.track),
      waits,
      fastest: waits.length ? Math.min(...waits) : null,
      slowest: waits.length ? Math.max(...waits) : null,
      totalTime: waits.length && waits.length === stats.length ? waits.reduce((sum, value) => sum + value, 0) : null,
      wrong: stats.filter(stat => stat.key === 'fail').map(stat => stat.rows[0]),
      shaky: stats.filter(stat => stat.key === 'mixed').map(stat => stat.rows[0]),
      errors: stats.filter(stat => stat.key === 'error').map(stat => stat.rows[0]),
      bySkill: Object.fromEntries(categories.map(key => [key, tally(system, inSkill(key))])),
      byLanguage: languages.map(item => tally(system, inLanguage(item))),
    };
  });
  const tracks = new Set(systems.map(system => system.track));
  const mock = [...tracks].every(track => track === 'mock_validation' || track === 'mock');
  const tries = Math.max(1, ...systems.map(system => system.tries));
  return {payload, ids, firstRow, categories, languages, systems, tracks, mock, tries, verdicts: payload.verdicts || []};
}

/* ---------- Sections ---------- */
function glanceHero(data) {
  const {payload, systems, ids, categories, tracks, mock} = data;
  const hero = node('header', undefined, 'hero g-hero'), title = node('h1'), chips = node('div', undefined, 'hero-chips');
  const kind = tracks.size === 1 ? gKind([...tracks][0]) : ['AI system', 'AI systems'];
  const indian = /india/i.test(payload.benchmark?.name || '');
  title.id = 'glance-title';
  if (mock) title.textContent = 'Test controls, not AI models.';
  else title.append(
    node('span', `${GLANCE_WORDS[systems.length] || systems.length} ${systems.length === 1 ? kind[0] : kind[1]}.`, 'g-line'),
    node('span', `${ids.length} everyday ${indian ? 'Indian ' : ''}${ids.length === 1 ? 'task' : 'tasks'}.`, 'g-line'));
  const lede = mock
    ? 'These runs check the evaluator itself. One control always gives the expected answer and the other only repeats the question, so the numbers below show the software working, not AI ability.'
    : `We gave ${gList(systems.map(system => system.short))} the same ${gPlural(ids.length, 'practical task')} on ${gList(categories.map(key => gSkill(key).short))}, then checked every answer. Here is what happened, in plain language.`;
  const started = payload.run?.started_at
    ? new Date(payload.run.started_at).toLocaleDateString('en-GB', {day: 'numeric', month: 'short', year: 'numeric', timeZone: 'Asia/Kolkata'}) + ' IST'
    : '';
  chips.append(node('span', 'Early test', 'chip chip-warn'), node('span', 'Not a ranking', 'chip'), node('span', [payload.benchmark?.name, started].filter(Boolean).join(' · '), 'hero-source'));
  hero.append(node('p', mock ? 'Software check' : 'AI models, explained simply', 'eyebrow'), title, node('p', lede, 'lede'), chips);
  return hero;
}

function glanceTakeaways(data) {
  const {systems, ids, languages, firstRow, tries} = data, total = ids.length, items = [];
  if (systems.length > 1 && total) {
    const score = system => tries > 1 ? system.accuracy ?? 0 : system.passed;
    const ranked = [...systems].sort((a, b) => score(b) - score(a)), high = ranked[0], low = ranked[ranked.length - 1];
    const marks = tries > 1
      ? `${high.short} was right on ${gPercent(high.accuracy)} of tries and ${low.short} on ${gPercent(low.accuracy)}.`
      : `${high.short} got ${high.passed} and ${low.short} got ${low.passed} of ${total} right.`;
    const verdict = gPair(data, high.id, low.id, 'accuracy');
    if (score(high) === score(low)) {
      items.push({icon: 'target', title: 'Same score', text: tries > 1 ? marks : `${systems.length === 2 ? 'Both' : 'All ' + systems.length} got ${high.passed} of ${total} tasks right.`});
    } else if (verdict) {
      items.push(verdict.verdict === 'clearly_different'
        ? {icon: 'target', title: 'A clear gap in accuracy', text: `${marks} The gap is too consistent to be luck.`}
        : {icon: 'target', title: 'Accuracy: too close to call', text: `${marks} Each task is worth about ${Math.round(100 / total)} marks out of 100, and a gap this small could be luck.`});
    } else {
      items.push({icon: 'target', title: 'Accuracy is close', text: `${marks} Each task is worth about ${Math.round(100 / total)} marks out of 100.`});
    }
  }
  const timed = systems.filter(system => typeof system.median === 'number' && system.median > 0).sort((a, b) => a.median - b.median);
  if (timed.length > 1) {
    const fast = timed[0], slow = timed[timed.length - 1], verdict = gPair(data, fast.id, slow.id, 'speed');
    const waits = `${fast.short} usually answered in about ${gAbout(fast.median)}; ${slow.short} usually took about ${gAbout(slow.median)}.`;
    if (verdict) {
      items.push(verdict.verdict === 'clearly_different' && verdict.favours
        ? {icon: 'gauge', title: 'Speed: clearly different', text: `${waits} Task by task, ${slow.short} took roughly ${gTimes(verdict.estimate)} as long.`}
        : {icon: 'gauge', title: 'Speed: too close to call', text: `${waits} The gap could be luck.`});
    } else {
      const ratio = slow.median / fast.median;
      items.push(ratio >= 1.5
        ? {icon: 'gauge', title: 'Speed is not close', text: `${waits} That is roughly ${gTimes(ratio)} as long.`}
        : {icon: 'gauge', title: 'Similar speed', text: `Typical waits: ${gList(timed.map(system => system.short + ' about ' + gAbout(system.median)))}.`});
    }
  }
  const slips = systems.flatMap(system => system.wrong);
  if (slips.length) {
    const counts = languages.map(item => ({
      item,
      slips: slips.filter(row => item.codes.includes(row.language)).length,
      tasks: ids.filter(id => item.codes.includes(firstRow(id).language)).length,
    })).filter(entry => entry.slips).sort((a, b) => b.slips - a.slips);
    const top = counts[0];
    if (top && (counts.length === 1 || top.slips > counts[1].slips)) {
      const name = top.item.name;
      let text = slips.length === 1 ? `The only wrong answer came from a ${name} task`
        : top.slips === slips.length ? `All ${slips.length} wrong answers came from ${name} tasks`
        : `${top.slips} of the ${slips.length} wrong answers came from ${name} tasks`;
      text += top.slips / slips.length > top.tasks / total && top.tasks < total ? `, which were only ${top.tasks} of the ${total}.` : '.';
      items.push({icon: 'speech', title: (top.slips === slips.length ? 'Every slip was in ' : 'Most slips were in ') + name, text});
    } else if (counts.length) {
      items.push({icon: 'speech', title: 'Where the slips were', text: gList(counts.map(entry => `${entry.slips} in ${entry.item.name}`)) + '.'});
    }
  } else if (systems.length && total) {
    items.push({icon: 'speech', title: 'No wrong answers', text: 'Every task was answered correctly, in every language.'});
  }
  const wrap = node('div', undefined, 'g-takeaways');
  wrap.setAttribute('aria-label', 'Key takeaways');
  for (const item of items) {
    const card = node('article', undefined, 'g-take');
    card.append(gIcon(item.icon), node('h3', item.title), node('p', item.text));
    wrap.append(card);
  }
  return items.length ? wrap : null;
}

// "Is the difference real?": each verdict as a third-umpire decision plus a margin-of-error line.
// Position on the line is signed so that the right-hand side always means runner_a is ahead.
function gMargin(v, name) {
  let estimate, low, high;
  if (v.metric === 'accuracy') {
    estimate = v.estimate;
    [low, high] = v.ci_95 || [estimate, estimate];
  } else {
    const sign = v.favours === v.runner_b ? -1 : 1, [first, last] = v.ci_95 || [v.estimate, v.estimate];
    estimate = sign * Math.log(v.estimate);
    [low, high] = sign > 0 ? [Math.log(first), Math.log(last)] : [-Math.log(last), -Math.log(first)];
  }
  const reach = 1.1 * Math.max(Math.abs(low), Math.abs(high), Math.abs(estimate), v.metric === 'accuracy' ? 10 : Math.log(2));
  const at = value => 50 + value / reach * 50;
  const line = node('div', undefined, 'g-margin'), track = node('div', undefined, 'g-margin-track'), band = node('i', undefined, 'g-margin-band');
  const dot = node('b', undefined, 'g-margin-dot'), ends = node('div', undefined, 'g-margin-ends'), ahead = v.metric === 'accuracy' ? ' ahead' : ' faster';
  band.style.left = at(Math.min(low, high)) + '%';
  band.style.width = Math.max(at(Math.max(low, high)) - at(Math.min(low, high)), 0.6) + '%';
  dot.style.left = at(estimate) + '%';
  track.append(node('span', undefined, 'g-margin-mid'), band, dot);
  ends.append(node('span', name(v.runner_b) + ahead), node('span', v.metric === 'accuracy' ? 'No gap' : 'Same speed'), node('span', name(v.runner_a) + ahead));
  line.setAttribute('role', 'img');
  line.setAttribute('aria-label', `Likely range ${low < 0 && high > 0 ? 'crosses' : 'does not cross'} the middle line`);
  line.append(track, ends);
  return line;
}
function gVerdictText(v, name) {
  const a = name(v.runner_a), b = name(v.runner_b);
  if (v.metric === 'accuracy') {
    const parts = [];
    if (v.a_better) parts.push(`${a} did better on ${gPlural(v.a_better, 'task')}`);
    if (v.b_better) parts.push(v.a_better ? `${b} on ${v.b_better}` : `${b} did better on ${gPlural(v.b_better, 'task')}`);
    const split = parts.length ? parts.join(' and ') + (v.tied ? `, and ${v.tied} were tied.` : '.') : `They matched on all ${gPlural(v.tasks, 'task')}.`;
    if (v.verdict === 'clearly_different') return `${split} That is too one-sided to be luck: ${name(v.favours)} is ahead by about ${Math.round(Math.abs(v.estimate))} marks out of 100.`;
    if (!v.ci_95) return `${split} That could easily be luck.`;
    const side = value => value < 0 ? `${b} ahead by ${Math.round(-value)}` : value > 0 ? `${a} ahead by ${Math.round(value)}` : 'no gap';
    return `${split} That could easily be luck: the true gap could be anywhere from ${side(v.ci_95[0])} to ${side(v.ci_95[1])} marks out of 100.`;
  }
  if (!v.favours) return `Their typical waits matched on all ${gPlural(v.tasks, 'task')}.`;
  const fast = name(v.favours), slow = v.favours === v.runner_a ? b : a, wins = v.favours === v.runner_a ? v.a_better : v.b_better;
  const split = wins === v.tasks ? `${fast} was faster on all ${gPlural(v.tasks, 'task')}.` : `${fast} was faster on ${wins} of ${gPlural(v.tasks, 'task')}.`;
  if (v.verdict !== 'clearly_different') return `${split} That could be luck, so we can't say which is faster yet.`;
  const range = v.ci_95 ? ` (likely between ${v.ci_95[0].toFixed(1)} and ${v.ci_95[1].toFixed(1)} times)` : '';
  return `${split} ${slow} usually took about ${gTimes(v.estimate)} as long${range}.`;
}
function glanceVerdicts(data) {
  const present = id => data.systems.some(system => system.id === id);
  const verdicts = data.verdicts.filter(v => present(v.runner_a) && present(v.runner_b));
  if (!verdicts.length) return null;
  const section = gSection('Third umpire', 'Is the difference real?', 'With a small test, a gap can be plain luck. Like a third umpire, we only give a decision when the replay is clear. The shaded band shows where the true gap probably lies; if it crosses the middle line, it is too close to call.', 'verdicts');
  const grid = node('div', undefined, 'g-verdicts'), name = id => data.systems.find(system => system.id === id)?.short || id;
  for (const v of verdicts) {
    const clear = v.verdict === 'clearly_different', card = node('article', undefined, 'card g-verdict'), head = node('div', undefined, 'g-verdict-head');
    head.append(gIcon(v.metric === 'accuracy' ? 'target' : 'gauge'), node('h3', (v.metric === 'accuracy' ? 'Accuracy' : 'Speed') + (data.systems.length > 2 ? ': ' + name(v.runner_a) + ' vs ' + name(v.runner_b) : '')));
    const p = v.p_value_adjusted < 0.001 ? '< 0.001' : '= ' + v.p_value_adjusted.toFixed(2);
    card.append(head, node('p', clear ? 'Clearly different' : 'Too close to call', 'g-decision' + (clear ? ' g-clear' : '')), gMargin(v, name),
      node('p', gVerdictText(v, name)), node('p', `Exact sign test over ${gPlural(v.tasks, 'task')}${v.tries_per_task > 1 ? ', ' + v.tries_per_task + ' tries each' : ''} · p ${p}`, 'g-note'));
    grid.append(card);
  }
  section.append(grid);
  return section;
}

function glanceReportCards(data) {
  const repeated = data.tries > 1;
  const section = gSection('Report cards', 'How many did each one get right?', 'Marks by subject, like a school report card. The bars work like phone signal: one bar for each task in that subject, filled when the answer was correct.' + (repeated ? ` Each task was tried ${data.tries} times: a full bar means right every time, a half bar right on some tries.` : ''), 'marks');
  const cards = node('div', undefined, 'g-cards');
  for (const system of data.systems) {
    const card = node('article', undefined, 'card g-card'), id = node('div', undefined, 'g-id'), names = node('div');
    names.append(node('h3', system.short), node('p', system.kind[0].replace(/^./, letter => letter.toUpperCase()) + (system.version ? ' · version ' + system.version : '')), node('p', 'AI model inside: ' + (system.model || system.id), 'g-model'));
    id.append(gAvatar(system), names);
    const score = node('div', undefined, 'g-score');
    if (system.tries > 1) score.append(node('strong', gPercent(system.accuracy)), node('span', 'of tries right'), node('span', `${system.passed}/${system.total} every time`, 'g-pct'));
    else score.append(node('strong', String(system.passed)), node('span', '/' + system.total + ' correct'), node('span', system.total ? Math.round(system.passed / system.total * 100) + '%' : '—', 'g-pct'));
    const table = node('table', undefined, 'g-marks'), thead = node('thead'), head = node('tr'), body = node('tbody');
    table.append(node('caption', system.short + ': marks by subject', 'sr-only'));
    for (const text of ['Subject', 'Marks']) {
      const cell = node('th', text);
      cell.scope = 'col';
      head.append(cell);
    }
    thead.append(head);
    for (const key of data.categories) {
      const skill = gSkill(key), tally = system.bySkill[key], row = node('tr', undefined, tally.passed < tally.total ? 'g-low' : '');
      const subject = node('th'), name = node('span', undefined, 'g-subject'), marks = node('td');
      subject.scope = 'row';
      name.append(gIcon(skill.icon), node('span', skill.name));
      subject.append(name);
      marks.append(gSignal(tally.passed, tally.total, `${tally.passed} of ${tally.total} right every time` + (tally.sometimes ? `, ${tally.sometimes} sometimes` : ''), tally.sometimes), gMarks(tally));
      row.append(subject, marks);
      body.append(row);
    }
    table.append(thead, body);
    const slips = node('p', undefined, 'g-slips');
    const lines = [[system.tries > 1 ? 'Never right: ' : 'Got wrong: ', system.wrong], ['Right only sometimes: ', system.shaky], ['Could not finish: ', system.errors]].filter(([, rows]) => rows.length);
    if (!lines.length) slips.append(node('b', system.tries > 1 ? 'Right on every try of every task.' : 'Got every task right.'));
    lines.forEach(([label, rows], index) => {
      if (index) slips.append(document.createElement('br'));
      slips.append(node('b', label), document.createTextNode(rows.map(row => gTitle(row.task_id)).join('; ')));
    });
    card.append(id, score, table, slips);
    cards.append(card);
  }
  section.append(cards);
  return section;
}

// A car speedometer. The dial reads answers per minute, so further right means faster; every
// dial shares one scale so the needles can be compared directly.
function gScale(systems) {
  const rates = systems.filter(system => system.median > 0).map(system => Math.min(60 / system.median, 120));
  const top = Math.max(1, ...rates) * 1.15;
  const max = [2, 5, 10, 15, 20, 30, 40, 60, 80, 100, 120].find(value => value >= top) || 120;
  return {max, step: {2: 0.5, 5: 1, 10: 2, 15: 5, 20: 5, 30: 10, 40: 10, 60: 20, 80: 20, 100: 20, 120: 30}[max]};
}
function gGauge(system, scale) {
  const cx = 120, cy = 124, r = 100, rate = system.median > 0 ? 60 / system.median : system.median === 0 ? Infinity : null;
  const fraction = rate === null ? 0 : Math.min(rate / scale.max, 1);
  const at = (part, radius) => {
    const angle = (210 - 240 * part) * Math.PI / 180;
    return [cx + radius * Math.cos(angle), cy - radius * Math.sin(angle)];
  };
  const arc = (from, to, radius) => {
    const [x0, y0] = at(from, radius), [x1, y1] = at(to, radius);
    return `M${x0.toFixed(2)} ${y0.toFixed(2)}A${radius} ${radius} 0 ${(to - from) * 240 > 180 ? 1 : 0} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
  };
  const svg = gSvg('svg', {viewBox: '0 0 240 178', class: 'g-gauge', role: 'img',
    'aria-label': `${system.short} speedometer: ${rate === null ? 'not measured' : gRateText(rate).toLowerCase()}`});
  svg.append(gSvg('path', {d: arc(0, 1, r - 9), class: 'band'}));
  if (rate !== null && fraction > 0.004) svg.append(gSvg('path', {d: arc(0, fraction, r - 9), class: 'sweep'}));
  svg.append(gSvg('path', {d: arc(0, 1, r), class: 'rim'}));
  const minor = scale.step / 5, count = Math.round(scale.max / minor);
  for (let index = 0; index <= count; index++) {
    const part = index / count, major = index % 5 === 0, [x0, y0] = at(part, r), [x1, y1] = at(part, r - (major ? 16 : 8));
    svg.append(gSvg('line', {x1: x0.toFixed(2), y1: y0.toFixed(2), x2: x1.toFixed(2), y2: y1.toFixed(2), class: major ? 'tick major' : 'tick'}));
    if (major) {
      const value = index * minor, [tx, ty] = at(part, r - 31), text = gSvg('text', {x: tx.toFixed(2), y: (ty + 4).toFixed(2), class: 'num'});
      text.textContent = Number.isInteger(value) ? String(value) : value.toFixed(1);
      svg.append(text);
    }
  }
  const unit = gSvg('text', {x: cx, y: cy + 44, class: 'unit'});
  unit.textContent = 'answers / min';
  svg.append(unit);
  if (rate !== null) {
    const needle = gSvg('path', {d: `M${cx - 4} ${cy + 16}L${cx - 1.2} ${cy - r + 14}L${cx + 1.2} ${cy - r + 14}L${cx + 4} ${cy + 16}Z`, class: 'needle'});
    needle.style.transform = `rotate(${(-120 + 240 * fraction).toFixed(2)}deg)`;
    svg.append(needle);
  }
  svg.append(gSvg('circle', {cx, cy, r: 9, class: 'hub'}), gSvg('circle', {cx, cy, r: 3, class: 'hub-dot'}));
  return svg;
}
function glanceSpeed(data) {
  const section = gSection('Speed', 'How fast does it answer?', 'Read it like a speedometer: the further the needle swings right, the more answers you get in a minute. Each wait was timed from asking a task to getting the answer back.', 'speed');
  const scale = gScale(data.systems), gauges = node('div', undefined, 'g-gauges');
  for (const system of data.systems) {
    const card = node('figure', undefined, 'card g-gauge-card'), readout = node('figcaption', undefined, 'g-readout');
    if (typeof system.median === 'number') {
      const band = gBand(system.median), rate = system.median > 0 ? 60 / system.median : Infinity;
      readout.append(node('span', band.label, 'g-band'), node('strong', gWait(system.median)), node('span', 'typical wait for one answer', 'g-sub'),
        node('p', band.like, 'g-like'), node('p', gRateText(rate) + (rate > scale.max ? ' · off the dial' : ''), 'g-rate'));
    } else {
      readout.append(node('strong', '—'), node('span', 'Wait time not measured', 'g-sub'));
    }
    card.append(gWho(system), gGauge(system, scale), readout);
    gauges.append(card);
  }
  section.append(gauges);
  const timed = data.systems.filter(system => typeof system.totalTime === 'number');
  if (timed.length) {
    const block = node('div', undefined, 'g-block'), longest = Math.max(...timed.map(system => system.totalTime)), list = node('div', undefined, 'g-trips');
    block.append(node('h3', `All ${gPlural(data.ids.length, 'task')}, back to back`), node('p', 'Like a download bar: the time each one needed to finish every task, one after another.', 'g-note'));
    for (const system of timed) {
      const row = node('div', undefined, 'g-trip'), bar = node('div', undefined, 'g-trip-bar'), fill = node('i'), text = node('p');
      fill.style.setProperty('--share', String(longest > 0 ? Math.max(system.totalTime / longest, 0.01) : 0.01));
      bar.setAttribute('role', 'img');
      bar.setAttribute('aria-label', `${system.short}: ${gClock(system.totalTime)} for all tasks`);
      bar.append(fill);
      text.append(node('b', gClock(system.totalTime)), document.createTextNode(' · ' + gTrip(system.totalTime)));
      row.append(gWho(system, true), bar, text);
      list.append(row);
    }
    block.append(list);
    section.append(block);
  }
  const waited = data.systems.filter(system => system.waits.length);
  if (waited.length) {
    const block = node('div', undefined, 'g-block'), list = node('div', undefined, 'g-waits');
    const ticks = axisTicks(Math.max(...waited.flatMap(system => system.waits))), top = ticks[ticks.length - 1];
    block.append(node('h3', 'Every wait, task by task'), node('p', 'Each dot is one task' + (data.tries > 1 ? ', at its middle wait across tries' : '') + '. A tight cluster means the wait is predictable; a wide spread means it changes from task to task. A cross marks a wrong answer.', 'g-note'));
    for (const system of waited) {
      const row = node('div', undefined, 'g-wait'), strip = node('div', undefined, 'g-strip');
      strip.setAttribute('role', 'img');
      strip.setAttribute('aria-label', `${system.short}: waits from ${gWait(system.fastest)} to ${gWait(system.slowest)}`);
      for (const stat of system.tasks.values()) {
        if (typeof stat.wait !== 'number') continue;
        const wrong = stat.key === 'fail', dot = node('i', undefined, wrong ? 'fail' : '');
        dot.style.left = (top > 0 ? stat.wait / top * 100 : 0) + '%';
        dot.title = gTitle(stat.rows[0].task_id) + ' · ' + gWait(stat.wait) + (wrong ? ' · wrong answer' : '');
        strip.append(dot);
      }
      const who = node('div', undefined, 'g-wait-who');
      who.append(gWho(system, true), node('p', `${gWait(system.fastest)} to ${gWait(system.slowest)}`, 'g-note'));
      row.append(who, strip);
      list.append(row);
    }
    const axisRow = node('div', undefined, 'g-wait g-axis-row');
    axisRow.append(node('span'), plotAxis(ticks, top, 's'));
    list.append(axisRow);
    block.append(list);
    section.append(block);
  }
  return section;
}

// A chat replay: both assistants "type" for exactly the wait measured on the chosen task, then
// show whether that answer was right. Answer text is never shown; it is not in the payload.
function glanceChat(data) {
  if (!data.systems.length || !data.ids.length) return null;
  const section = gSection('Replay', 'Ask them at the same time', 'Pick a task and press Ask, like sending one message to several chats. Each reply arrives after the real wait measured for that task and shows whether the answer was right. The answers themselves stay private.', 'chat');
  const controls = node('div', undefined, 'g-chat-controls'), field = node('div', undefined, 'g-field'), picker = node('select');
  const fieldLabel = node('label', 'Task to send', 'field-label');
  picker.id = 'glance-chat-task';
  fieldLabel.htmlFor = picker.id;
  for (const key of data.categories) {
    const group = document.createElement('optgroup');
    group.label = gSkill(key).name;
    for (const id of data.ids.filter(id => (data.firstRow(id).category || 'other') === key)) {
      const option = node('option', gTitle(id));
      option.value = id;
      group.append(option);
    }
    picker.append(group);
  }
  field.append(fieldLabel, picker);
  const tryPicker = node('select');
  tryPicker.id = 'glance-chat-try';
  for (let index = 0; index < data.tries; index++) {
    const option = node('option', `Try ${index + 1} of ${data.tries}`);
    option.value = String(index);
    tryPicker.append(option);
  }
  const tryField = node('div', undefined, 'g-field g-field-try'), tryLabel = node('label', 'Which try', 'field-label');
  tryLabel.htmlFor = tryPicker.id;
  tryField.append(tryLabel, tryPicker);
  tryField.hidden = data.tries < 2;
  const ask = node('button', data.systems.length === 1 ? 'Ask' : data.systems.length === 2 ? 'Ask both' : 'Ask all', 'primary');
  const skip = node('button', 'Skip the wait', 'secondary'), watch = node('span', '0.0 s', 'g-stopwatch');
  ask.type = skip.type = 'button';
  skip.hidden = true;
  watch.setAttribute('aria-hidden', 'true');
  controls.append(field, tryField, ask, skip, watch);
  const windows = node('div', undefined, 'g-chats');
  const chats = data.systems.map(system => {
    const card = node('article', undefined, 'g-chat'), head = node('header', undefined, 'g-chat-head'), name = node('div');
    const status = node('span', 'online', 'g-chat-status'), log = node('div', undefined, 'g-chat-log');
    name.append(node('strong', system.short), status);
    head.append(gAvatar(system, true), name);
    log.setAttribute('role', 'log');
    log.setAttribute('aria-live', 'polite');
    log.setAttribute('aria-label', system.short + ' chat');
    log.append(node('p', 'Press Ask to send the task.', 'g-chat-empty'));
    card.append(head, log);
    windows.append(card);
    return {system, status, log, row: null, done: true};
  });
  let started = 0;
  const halt = () => {
    for (const timer of glance.chatTimers) clearTimeout(timer);
    glance.chatTimers = [];
    cancelAnimationFrame(glance.chatFrame);
  };
  const tick = () => {
    watch.textContent = ((performance.now() - started) / 1000).toFixed(1) + ' s';
    glance.chatFrame = requestAnimationFrame(tick);
  };
  const reply = chat => {
    chat.log.querySelector('.g-typing')?.remove();
    const result = outcome(chat.row), wait = chat.row?.latency_s, bubble = node('div', undefined, 'g-msg g-msg-in' + (result.key === 'fail' ? ' g-wrong' : ''));
    const text = {pass: 'Answered correctly', fail: 'Gave a wrong answer', error: 'Could not answer'}[result.key] || 'Not graded yet';
    bubble.append(gIcon(result.key === 'pass' ? 'check' : 'cross'), node('span', text), node('small', typeof wait === 'number' ? gWait(wait) : 'time not measured'));
    chat.log.append(bubble);
    chat.status.textContent = typeof wait === 'number' ? 'replied in ' + gWait(wait) : 'replied';
    chat.done = true;
    if (chats.every(other => other.done)) {
      cancelAnimationFrame(glance.chatFrame);
      const waits = chats.map(other => other.row?.latency_s).filter(value => typeof value === 'number');
      watch.textContent = waits.length ? gWait(Math.max(...waits)) : '—';
      skip.hidden = true;
      ask.disabled = false;
    }
  };
  ask.onclick = () => {
    halt();
    const id = picker.value;
    started = performance.now();
    ask.disabled = true;
    skip.hidden = false;
    for (const chat of chats) {
      const question = node('div', undefined, 'g-msg g-msg-out'), typing = node('div', undefined, 'g-msg g-msg-in g-typing');
      question.append(node('span', gTitle(id)));
      typing.setAttribute('aria-label', chat.system.short + ' is typing');
      typing.append(node('i'), node('i'), node('i'));
      chat.log.replaceChildren(question, typing);
      chat.status.textContent = 'typing…';
      chat.row = chat.system.rows.find(row => row.task_id === id && (row.repeat_index || 0) === Number(tryPicker.value)) || null;
      chat.done = false;
      const wait = typeof chat.row?.latency_s === 'number' ? chat.row.latency_s : 0;
      glance.chatTimers.push(setTimeout(() => reply(chat), wait * 1000));
    }
    glance.chatFrame = requestAnimationFrame(tick);
  };
  skip.onclick = () => {
    halt();
    for (const chat of chats) if (!chat.done) reply(chat);
  };
  section.append(controls, windows);
  return section;
}

function glanceLanguages(data) {
  if (data.languages.length < 2) return null;
  const indic = data.languages.some(item => item.codes.includes('hi') || item.codes.includes('hi-Latn'));
  const section = gSection('Languages', indic ? 'Does it understand Hindi and Hinglish?' : 'How did it do in each language?', 'The same everyday request can be asked in different ways. One dot per task: filled means it answered correctly.', 'languages');
  const grid = node('div', undefined, 'g-langs');
  data.languages.forEach((item, index) => {
    const card = node('article', undefined, 'card g-lang'), heading = node('h3'), nativeName = node('span', item.native);
    const count = data.ids.filter(id => item.codes.includes(data.firstRow(id).language)).length;
    if (item.sample) {
      const bubble = node('p', item.sample, 'g-bubble');
      if (item.lang) bubble.lang = item.lang;
      card.append(bubble);
    }
    if (item.lang) nativeName.lang = item.lang;
    heading.append(nativeName);
    if (item.native !== item.name) heading.append(node('small', item.name));
    card.append(heading, node('p', gPlural(count, 'task') + (item.sample ? ' · the bubble is an example, not a test question' : ''), 'g-note'));
    for (const system of data.systems) {
      const tally = system.byLanguage[index], row = node('div', undefined, 'g-lang-row'), who = node('div');
      who.append(node('span', system.short, 'g-lang-name'), gDots(tally.passed, tally.total, `${tally.passed} of ${tally.total} right every time` + (tally.sometimes ? `, ${tally.sometimes} sometimes` : ''), tally.sometimes));
      row.append(gAvatar(system, true), who, node('b', tally.passed + '/' + tally.total));
      card.append(row);
    }
    grid.append(card);
  });
  section.append(grid);
  return section;
}

function gTallyText(tally) {
  return `${tally.passed} of ${tally.total}` + (tally.sometimes ? ` (+${tally.sometimes} sometimes)` : '');
}
function glanceSpecs(data) {
  const {systems, ids, categories, languages} = data;
  const section = gSection('Side by side', systems.length > 1 ? 'Compare them like phones' : 'The key numbers', 'All the numbers in one table, the way phone shops compare models. A bar on the left marks a row where they differ.', 'specs');
  const groups = [
    ['Results', data.tries > 1 ? [
      ['Share of tries right', system => gPercent(system.accuracy)],
      ['Right every time', system => `${system.passed} of ${system.total} tasks`],
      ['Right only sometimes', system => `${system.sometimes} of ${system.total} tasks`],
    ] : [['Correct answers', system => `${system.passed} of ${system.total}` + (system.total ? ` (${Math.round(system.passed / system.total * 100)}%)` : '')]]],
    ['Speed', [
      ['Typical wait', system => gWait(system.median)],
      ['Fastest answer', system => gWait(system.fastest)],
      ['Slowest answer', system => gWait(system.slowest)],
      [`All ${gPlural(ids.length, 'task')}, back to back`, system => typeof system.totalTime === 'number' ? gClock(system.totalTime) : '—'],
    ]],
    ['Subjects', categories.map(key => [gSkill(key).name, system => gTallyText(system.bySkill[key])])],
    ['Languages', languages.length > 1 ? languages.map((item, index) => [item.name, system => gTallyText(system.byLanguage[index])]) : []],
    ['About', [
      [data.tries > 1 ? 'Never right' : 'Got wrong', system => system.wrong.length ? system.wrong.map(row => gTitle(row.task_id)).join('; ') : 'Nothing'],
      ...(data.tries > 1 ? [['Right only sometimes', system => system.shaky.length ? system.shaky.map(row => gTitle(row.task_id)).join('; ') : 'Nothing']] : []),
      ['AI model inside', system => system.model || system.id],
      ['What it is', system => system.kind[0].replace(/^./, letter => letter.toUpperCase()) + ' · ' + system.kind[2].toLowerCase()],
    ]],
  ];
  const table = node('table', undefined, 'g-specs' + (glance.onlyDiff ? ' only-diff' : '')), thead = node('thead'), head = node('tr'), body = node('tbody');
  const corner = node('th');
  corner.append(node('span', 'Measure', 'sr-only'));
  corner.scope = 'col';
  head.append(corner);
  for (const system of systems) {
    const cell = node('th');
    cell.scope = 'col';
    cell.append(gWho(system));
    head.append(cell);
  }
  thead.append(head);
  let differences = 0, rowsTotal = 0;
  for (const [title, rows] of groups) {
    if (!rows.length) continue;
    const group = node('tr', undefined, 'group'), cell = node('th', title);
    cell.colSpan = systems.length + 1;
    cell.scope = 'colgroup';
    group.append(cell);
    body.append(group);
    let groupDiffers = false;
    for (const [name, value] of rows) {
      const values = systems.map(value), differs = systems.length > 1 && new Set(values).size > 1, row = node('tr', undefined, differs ? 'diff' : ''), heading = node('th', name);
      heading.scope = 'row';
      row.append(heading, ...values.map(text => node('td', text)));
      body.append(row);
      rowsTotal++;
      if (differs) {
        differences++;
        groupDiffers = true;
      }
    }
    if (!groupDiffers) group.classList.add('no-diff');
  }
  table.append(thead, body);
  if (systems.length > 1) {
    const tools = node('div', undefined, 'g-tools'), seg = node('div', undefined, 'seg');
    seg.setAttribute('role', 'group');
    seg.setAttribute('aria-label', 'Rows to show');
    for (const [only, text, count] of [[false, 'All rows', rowsTotal], [true, 'Only differences', differences]]) {
      const button = node('button', text);
      button.type = 'button';
      button.append(node('span', String(count)));
      button.setAttribute('aria-pressed', String(glance.onlyDiff === only));
      button.disabled = only && !differences;
      button.onclick = () => {
        glance.onlyDiff = only;
        table.classList.toggle('only-diff', only);
        for (const other of seg.children) other.setAttribute('aria-pressed', String(other === button));
      };
      seg.append(button);
    }
    tools.append(seg);
    section.append(tools);
  }
  const scroll = node('div', undefined, 'table-scroll g-specs-wrap');
  scroll.append(table);
  section.append(scroll);
  return section;
}

function glanceFinePrint(data) {
  const {payload, ids} = data;
  const section = gSection('Before you decide', 'Read this before you choose', '', 'fine');
  const oneTry = (payload.pilot?.conditions || []).some(text => /one attempt/i.test(text));
  const list = node('ul', undefined, 'g-fine');
  for (const text of [
    `This is an early test: ${gPlural(ids.length, 'task')}${data.tries > 1 ? `, ${data.tries} tries each` : oneTry || data.tries === 1 ? ', one try each' : ''}. A bigger test could change the picture.`,
    data.tries > 1 ? 'AI answers can change from one try to the next. A half bar or half dot marks a task answered correctly on some tries only.' : 'AI answers can change from one try to the next. Running each task several times shows how consistent a model is.',
    ids.length ? `Each task is worth about ${Math.round(100 / ids.length)} marks out of 100, so small gaps matter less than they look.` : '',
    'Wait times include starting up and sending the answer back, timed on the computer that ran the test. A chat app on your phone may feel different.',
    'Not a ranking. The tasks still need independent review before anyone treats these results as final.',
  ]) if (text) list.append(node('li', text));
  section.append(list);
  const notes = [...(payload.limitations || []), ...(payload.pilot?.conditions || [])];
  if (notes.length) {
    const details = node('details', undefined, 'g-tech'), items = node('ul');
    details.append(node('summary', `Technical notes (${notes.length})`));
    for (const text of notes) items.append(node('li', text));
    details.append(items);
    section.append(details);
  }
  const actions = node('div', undefined, 'g-actions');
  for (const [text, view, className] of [['See the detailed results', 'board', 'primary'], ['How we tested', 'overview', 'secondary']]) {
    const button = node('button', text, className);
    button.type = 'button';
    button.dataset.view = view;
    actions.append(button);
  }
  section.append(actions);
  return section;
}

/* ---------- Entry points (called from app.js) ---------- */
function drawGlance() {
  const target = $('glance');
  if (!target) return;
  for (const timer of glance.chatTimers) clearTimeout(timer);
  glance.chatTimers = [];
  cancelAnimationFrame(glance.chatFrame);
  target.replaceChildren();
  const payload = glance.payload;
  if (!payload || !payload.results?.length) {
    const empty = node('div', undefined, 'card empty'), go = node('button', 'Go to Run', 'primary');
    go.type = 'button';
    go.dataset.view = 'dashboard';
    empty.append(node('h2', 'Nothing to show yet'), node('p', 'Run an evaluation and the results will appear here, explained in plain language.'), go);
    target.append(empty);
    return;
  }
  glance.titled = state.tasks.length > 0;
  const data = glanceData(payload, glance.adapters);
  target.append(glanceHero(data));
  if (data.mock) {
    const note = node('p', undefined, 'mock-note');
    note.append(node('strong', 'Test controls'), document.createTextNode('These are software checks, not AI models. Run real models from the Run tab to compare them here.'));
    target.append(note);
  } else {
    const takeaways = glanceTakeaways(data);
    if (takeaways) target.append(takeaways);
  }
  for (const section of [glanceVerdicts(data), glanceReportCards(data), glanceSpeed(data), glanceChat(data), glanceLanguages(data), glanceSpecs(data), glanceFinePrint(data)]) {
    if (section) target.append(section);
  }
}
function renderGlance(payload, job, adapters) {
  Object.assign(glance, {payload, job, adapters});
  drawGlance();
}
// Task titles load separately; redraw once if the first draw had to fall back to task IDs.
function refreshGlance() {
  if (glance.payload && !glance.titled) drawGlance();
}
