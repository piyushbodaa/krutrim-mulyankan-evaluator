"use strict";
/* English is the source. Hindi covers the shell. The honesty labels stay in English in both. */
const I18N = {
  en: {
    "edition.place": "India-focused AI evaluation",
    "welcome.kicker": "On your computer",
    "welcome.title": "Check an AI model without a terminal.",
    "welcome.body": "Add keys for the providers you use, pick models and a test pack, look at the cost, then run. Results open in the same At a glance view, with the third umpire.",
    "welcome.start": "Get started",
    "welcome.note": "Practice checks need no key and cost nothing. They are not a ranking.",
    "keys.title": "Add API keys",
    "keys.body": "Keys are stored in the system keychain, not in a file. Skip this if you only want the free practice checks.",
    "keys.skip": "Skip for now",
    "keys.next": "Continue",
    "keys.get": "Get a key",
    "keys.save": "Save key",
    "keys.saved": "Saved in the system keychain.",
    "keys.session": "Saved for this session, in memory only.",
    "keys.forget": "Remove",
    "keys.placeholder": "Paste the key",
    "models.title": "Pick what to check",
    "models.body": "Practice checks are free. A priced model can spend money, and only up to the cap you set next.",
    "models.next": "Choose a test pack",
    "models.practice": "Practice checks",
    "models.priced": "Priced models",
    "models.later": "Not ready yet",
    "pack.title": "Pick a test pack",
    "pack.body": "The pilot set is the place to start.",
    "pack.next": "See the cost",
    "pack.tasks": "tasks",
    "cost.title": "Cost before anything runs",
    "cost.body": "This is the worst case, not a bill. It assumes the whole prompt goes in and the answer uses the full length allowed.",
    "cost.cap": "Spending cap (USD)",
    "cost.repeats": "Tries per task",
    "cost.run": "Run",
    "cost.fx": "Rupee amounts use the rate stored in the app, not a live bank rate.",
    "progress.title": "Running",
    "progress.body": "The dial shows how much of this run is done. It is not a speed ranking.",
    "progress.cancel": "Cancel remaining",
    "progress.open": "Open results",
    "progress.waiting": "Starting…",
    "results.save": "Save JSON",
    "results.export": "Export",
    "results.share": "Share card",
    "results.again": "New check",
    "results.note": "At a glance and the third umpire use the same results view as the evaluator.",
    "common.back": "Back",
    "common.keys": "API keys",
    "common.loading": "Loading…",
    "common.suggested": "Suggested",
    "theme.system": "System",
    "theme.light": "Light",
    "theme.dark": "Dark",
    "confirm.free": "I understand these are practice checks. Unvalidated pilot. Not a ranking.",
    "confirm.paid": "I understand this can spend money up to the cap. It should not go over. Unvalidated pilot. Not a ranking.",
    "honesty.gloss": "Too close to call means the gap could be chance, not that they are equal.",
    "repeat.1": "1 try",
    "repeat.3": "3 tries",
    "repeat.5": "5 tries",
  },
  hi: {
    "edition.place": "भारत-केंद्रित एआई मूल्यांकन",
    "welcome.kicker": "आपके कंप्यूटर पर",
    "welcome.title": "टर्मिनल के बिना एआई मॉडल जाँचें।",
    "welcome.body": "जिन प्रदाताओं का आप उपयोग करते हैं उनकी कुंजी जोड़ें, मॉडल और टेस्ट पैक चुनें, खर्च देखें, फिर चलाएँ। नतीजे उसी एक नज़र दृश्य में खुलते हैं, तीसरे अंपायर के साथ।",
    "welcome.start": "शुरू करें",
    "welcome.note": "अभ्यास जाँच के लिए कुंजी नहीं चाहिए और खर्च शून्य है। यह रैंकिंग नहीं है।",
    "keys.title": "एपीआई कुंजी जोड़ें",
    "keys.body": "कुंजी सिस्टम कीचेन में रहती है, किसी फ़ाइल में नहीं। केवल मुफ़्त अभ्यास जाँच चाहिए तो इसे छोड़ दें।",
    "keys.skip": "अभी छोड़ें",
    "keys.next": "आगे",
    "keys.get": "कुंजी लें",
    "keys.save": "कुंजी सहेजें",
    "keys.saved": "सिस्टम कीचेन में सहेजी गई।",
    "keys.session": "इस सत्र के लिए मेमोरी में सहेजी गई।",
    "keys.forget": "हटाएँ",
    "keys.placeholder": "कुंजी चिपकाएँ",
    "models.title": "क्या जाँचना है",
    "models.body": "अभ्यास जाँच मुफ़्त है। कीमत वाला मॉडल पैसे खर्च कर सकता है, अगली स्क्रीन पर तय सीमा तक।",
    "models.next": "टेस्ट पैक चुनें",
    "models.practice": "अभ्यास जाँच",
    "models.priced": "कीमत वाले मॉडल",
    "models.later": "अभी तैयार नहीं",
    "pack.title": "टेस्ट पैक चुनें",
    "pack.body": "पायलट सेट से शुरू करें।",
    "pack.next": "खर्च देखें",
    "pack.tasks": "प्रश्न",
    "cost.title": "चलाने से पहले खर्च",
    "cost.body": "यह बिल नहीं, सबसे अधिक संभावित खर्च है। माना गया है कि पूरा प्रश्न जाएगा और उत्तर पूरी अनुमत लंबाई लेगा।",
    "cost.cap": "खर्च की सीमा (USD)",
    "cost.repeats": "हर प्रश्न पर कोशिशें",
    "cost.run": "चलाएँ",
    "cost.fx": "रुपये की रकम ऐप में दर्ज दर से है, बैंक की लाइव दर से नहीं।",
    "progress.title": "चल रहा है",
    "progress.body": "डायल बताती है कि यह रन कितना पूरा हुआ। यह गति की रैंकिंग नहीं है।",
    "progress.cancel": "बाकी रुकें",
    "progress.open": "नतीजे खोलें",
    "progress.waiting": "शुरू हो रहा है…",
    "results.save": "JSON सहेजें",
    "results.export": "निर्यात",
    "results.share": "कार्ड बाँटें",
    "results.again": "नई जाँच",
    "results.note": "एक नज़र और तीसरा अंपायर वही परिणाम दृश्य है।",
    "common.back": "पीछे",
    "common.keys": "एपीआई कुंजी",
    "common.loading": "लोड हो रहा है…",
    "common.suggested": "सुझाव",
    "theme.system": "सिस्टम",
    "theme.light": "हल्का",
    "theme.dark": "गहरा",
    "confirm.free": "मैं समझता/समझती हूँ कि ये अभ्यास जाँच हैं। Unvalidated pilot. Not a ranking.",
    "confirm.paid": "मैं समझता/समझती हूँ कि यह सीमा तक पैसे खर्च कर सकता है। इससे ऊपर नहीं जाना चाहिए। Unvalidated pilot. Not a ranking.",
    "honesty.gloss": "Too close to call का अर्थ है कि अंतर संयोग हो सकता है, बराबरी नहीं।",
    "repeat.1": "1 कोशिश",
    "repeat.3": "3 कोशिशें",
    "repeat.5": "5 कोशिशें",
  },
};

let kmLang = "en";
try {
  const saved = localStorage.getItem("km.desktop.lang");
  if (saved === "hi" || saved === "en") kmLang = saved;
} catch { /* English remains available. */ }

function t(key) {
  return (I18N[kmLang] && I18N[kmLang][key]) || I18N.en[key] || key;
}

function applyI18n() {
  document.documentElement.lang = kmLang === "hi" ? "hi" : "en";
  for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
  for (const el of document.querySelectorAll("[data-i18n-placeholder]")) {
    el.placeholder = t(el.dataset.i18nPlaceholder);
  }
  for (const button of document.querySelectorAll("[data-lang]")) {
    button.setAttribute("aria-pressed", String(button.dataset.lang === kmLang));
  }
}

function setLang(lang) {
  kmLang = lang === "hi" ? "hi" : "en";
  try { localStorage.setItem("km.desktop.lang", kmLang); } catch { /* The choice still applies. */ }
  applyI18n();
  document.dispatchEvent(new CustomEvent("km-lang"));
}
