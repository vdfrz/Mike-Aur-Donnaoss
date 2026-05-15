# MikeRust — Manuale di configurazione e utilizzo

> Manuale concettuale per l'uso quotidiano. Per la guida di setup ambiente
> (env vars, build, packaging Tauri) vedi [PLAN.md](../PLAN.md).
> Per la mappa upstream Mike vedi [docs/mike-upstream/README_SEMPLIFICA.md](mike-upstream/README_SEMPLIFICA.md).

---

## 1. Modello concettuale

MikeRust ruota attorno a **cinque entità** che l'utente vede e configura:

| Entità | Cos'è | Tabella DB |
|---|---|---|
| **Profilo utente** | Identità locale (username + PIN). Una sola per installazione. | `user_profiles` |
| **Documento** | File caricato dall'utente (PDF, DOCX, XLSX, immagini, …). | `documents` |
| **Progetto** | Cartella logica che raggruppa documenti e chat. | `projects` |
| **Chat** | Conversazione con l'assistente, dentro o fuori da un progetto. | `chats` + `messages` |
| **Workflow** | Template Markdown di un prompt riutilizzabile. | `workflows` |
| **Tabular Review** | Estrazione strutturata in tabella di dati da N documenti. | `tabular_reviews` + `tabular_review_rows` |

Tutte le entità sono **proprietà di un solo utente**: il backend verifica
`user_id == auth.user_id` prima di update/delete, il frontend mostra il modal
[OwnerOnlyModal.tsx](../frontend/src/app/components/shared/OwnerOnlyModal.tsx)
se la guard fallisce.

---

## 2. Configurazione LLM (`/account/models`)

Tutto parte da qui: senza un provider configurato, le chat falliscono.

### 2.1 Provider supportati

L'utente sceglie **uno fra questi** come `activeProvider`:

| Provider | Campo "Active provider" | Credenziali / endpoint richiesti |
|---|---|---|
| **OpenAI / ChatGPT** | `openai` | `openaiApiKey` + `openaiModel` (es. `gpt-4o`) |
| **Anthropic Claude** | `claude` | `claudeApiKey` (modello scelto da combo, presets `claude-opus-4-7`, `claude-sonnet-4-6`) |
| **Google Gemini** | `gemini` | `geminiApiKey` (presets `gemini-3.1-pro-preview`, `gemini-3-flash-preview`) |
| **Local / OpenAI-compatible** | `local` | `localBaseUrl` (es. `https://host/v1` oppure senza `/v1`, viene normalizzato), `localApiKey` (opzionale: vuoto per Ollama puro), `localModel` (es. `gemma3:12b`, `llama3`, `mistral`) |

### 2.2 Dove vengono salvate

Le impostazioni viaggiano per **due binari**:

1. **`localStorage` browser** sotto la chiave `mikerust_llm_settings`. Le legge il
   combo modelli della chat ([ModelToggle.tsx](../frontend/src/app/components/assistant/ModelToggle.tsx))
   per popolare le voci dinamicamente.
2. **DB SQLite** tabella `user_settings`, attraverso `PUT /user/llm-settings`.
   Le legge il backend al momento della chat per costruire `LocalConfig` o
   passare le API key ai client Claude/Gemini.

Premere **Save settings** invia entrambi.

### 2.3 Model id e prefissi nel combo chat

Il combo della chat compone i model id così:

| Provider configurato | Voce nel combo | id inviato al backend |
|---|---|---|
| Anthropic | preset Claude Opus / Sonnet | `claude-opus-4-7`, `claude-sonnet-4-6` |
| Google | preset Gemini Pro / Flash | `gemini-3.1-pro-preview`, `gemini-3-flash-preview` |
| OpenAI | il `openaiModel` configurato | `openai:<openaiModel>` |
| Local | il `localModel` configurato | `local:<localModel>` |

Il backend ([src/llm/mod.rs](../src/llm/mod.rs) `provider_for_model`)
riconosce i prefissi `openai:` / `local:` per dispatchare al client corretto;
`claude*` e `gemini*` vanno ai rispettivi client.

### 2.4 Modelli multimodali (vision)

Riconosciuti automaticamente da
[src/llm/mod.rs](../src/llm/mod.rs) `is_vision_capable`: pattern
`gemma3` · `gpt-4o` · `claude` · `gemini` · `llava` · `llama3.2-vision` ·
`qwen2-vl` · `qwen2.5-vl` · `pixtral` · `vision`.

Quando il modello selezionato è vision-capable:
- I PDF **scansionati** (testo non estraibile) vengono renderizzati a 200 DPI e inviati come immagini PNG (max 8 pagine — vedi `MAX_PDF_IMAGE_PAGES` in [src/routes/chat.rs](../src/routes/chat.rs)).
- I file PNG/JPG/JPEG vengono allegati direttamente.
- I TIFF (anche multi-pagina) vengono convertiti in JPEG q.85 e allegati.

Quando il modello **non** è vision-capable, le immagini vengono ignorate con un log di warning.

---

## 3. Documenti

### 3.1 Formati supportati

| Estensione | `file_type` DB | Trattamento backend |
|---|---|---|
| `.pdf` | `pdf` | Estrazione testo via pdfium (`libs/pdfium/pdfium.dll` deve essere bundled). Se `is_scanned_pdf` rileva la maggioranza delle pagine senza testo → fallback a render immagini per modelli vision. |
| `.docx` | `docx` | ZIP+XML pure-Rust ([src/pdf/mod.rs](../src/pdf/mod.rs) `extract_docx_text`). |
| `.xlsx`/`.xls`/`.xlsb`/`.ods` | come ext | calamine, una sezione per foglio, righe tab-separated. |
| `.csv`/`.txt`/`.md` | come ext | UTF-8 decode diretto. |
| `.png` | `png` | Allegato come `data:image/png;base64,…` (richiede modello vision). |
| `.jpg`/`.jpeg` | `jpeg` | Allegato come `data:image/jpeg;base64,…` (richiede modello vision). |
| `.tif`/`.tiff` | `tiff` | Decodifica multi-pagina via crate `tiff`, ogni frame ricodificato JPEG q.85. |

### 3.2 Storage fisico

Il file binario viene salvato in `STORAGE_PATH/documents/{user_id}/{document_id}`
(default `./data/storage`). Lo schema DB tiene solo metadati e `storage_path`.

### 3.3 Endpoint principali

```
GET    /document?project_id={id?}      lista doc (filtro opzionale)
POST   /document                       multipart: file (binary), project_id? (text)
GET    /document/{id}                  metadati + storage_path
DELETE /document/{id}                  rimuove anche da storage

# Alias upstream-Mike compat
POST   /single-documents               equivalente a POST /document
GET    /single-documents/{id}          equivalente a GET /document/{id}
```

### 3.4 Allegare un documento a una chat

Il messaggio inviato dal frontend ha questa forma:

```json
{
  "messages": [
    {
      "role": "user",
      "content": "analizza questo documento",
      "files": [
        { "filename": "report.pdf", "document_id": "<uuid>" }
      ]
    }
  ],
  "chat_id": "<uuid?>",
  "model": "local:gemma3:12b"
}
```

Il backend ([src/routes/chat.rs](../src/routes/chat.rs) `load_attached_docs`)
estrae i `document_id` da tutti i messaggi, li carica dallo storage, fa
extraction/conversion, e:
- aggrega i testi nel `system_prompt` (intestazione `=== Document: nome ===`);
- aggrega le immagini nel `messages[last_user].images` (multimodale).

---

## 4. Progetti

### 4.1 Cosa è

Un progetto è un **contenitore logico**. Permette di:
- Raggruppare documenti correlati;
- Creare chat che hanno automaticamente accesso ai documenti del progetto;
- Eseguire Tabular Reviews limitate al progetto.

### 4.2 Schema

Tabella `projects` ([0001_initial.sql:19-26](../migrations/0001_initial.sql#L19-L26)):
```
id           TEXT PK
user_id      TEXT FK → user_profiles.id (CASCADE)
name         TEXT NOT NULL
description  TEXT NULLABLE
created_at   TEXT (auto)
updated_at   TEXT (auto)
```

### 4.3 Cosa succede al delete

| FK | ON DELETE |
|---|---|
| `documents.project_id` | **SET NULL** — il documento sopravvive come "standalone". |
| `chats.project_id` | **CASCADE** — le chat del progetto vengono eliminate. |
| `tabular_reviews.project_id` | **SET NULL** — le review sopravvivono come standalone. |

### 4.4 Endpoint

```
GET    /project              lista per user_id, ordine updated_at DESC
POST   /project              body: { name, description? }   →  { id, name }
GET    /project/{id}         dettaglio
PUT    /project/{id}         body: { name?, description? }   (merge via COALESCE)
DELETE /project/{id}
```

---

## 5. Chat

### 5.1 Vita di una chat

1. **Creazione**: `POST /chat` con body `{ project_id?, title? }` (no `messages`) → ritorna `{ id }`.
2. **Invio messaggio**: `POST /chat` con body `{ messages, chat_id?, model }` → SSE stream. Se `chat_id` manca, viene creato e annunciato come primo evento `data: {"type":"chat_id","chatId":"…"}`.
3. **Generazione titolo**: `POST /chat/{id}/generate-title` (chiamato dal frontend dopo la prima risposta, usa lo stesso provider attivo).
4. **Cronologia**: `GET /chat/{id}` → `{ chat: {…}, messages: [{id, role, content, created_at}, …] }`.
5. **Eliminazione**: `DELETE /chat/{id}`.

### 5.2 Eventi SSE emessi

```
data: {"type":"chat_id","chatId":"…"}        // solo per nuove chat
data: {"type":"content_delta","text":"…"}    // 0..N volte durante streaming
data: {"type":"error","message":"…"}         // se LLM fallisce
data: {"type":"citations","citations":[]}    // sempre, marca fine stream
```

Il frontend ([useAssistantChat.ts](../frontend/src/app/hooks/useAssistantChat.ts)) li parsa e
costruisce gli `events` del messaggio assistente. Errori SSE sono propagati
come `Error` con flag `__mike_sse_error` per renderli visibili in UI.

---

## 6. Workflow

### 6.1 Cosa è

Un **template di prompt Markdown**. L'utente può scrivere un workflow tipo:
> *"Riassumi il documento allegato in 5 punti, evidenziando rischi e
> obblighi contrattuali. Restituisci in italiano."*

E poi richiamarlo nella chat (dal selettore "Workflows") senza riscrivere
ogni volta il prompt.

### 6.2 Schema

```
workflows:
  id, user_id, title, prompt_md, created_at, updated_at
```

### 6.3 Endpoint

```
GET    /workflow                lista
POST   /workflow                body: { title, prompt_md }
GET    /workflow/{id}
PUT    /workflow/{id}           body: { title?, prompt_md? }
DELETE /workflow/{id}

# Workflow nascosti per l'utente corrente
GET    /workflow/hidden         lista degli id nascosti
POST   /workflow/hidden         body: { workflow_id }
DELETE /workflow/hidden/{id}
```

### 6.4 Workflow "nascosti"

Tabella `workflow_hidden(user_id, workflow_id)`:
- L'utente può **nascondere** un workflow di sistema o condiviso senza eliminarlo.
- È utile quando non si vuole un workflow nel selettore quotidiano ma non si vuole perderlo.

---

## 7. Tabular Reviews

### 7.1 Cosa è

Un **foglio di calcolo dinamico** in cui:
- Le **righe** sono documenti;
- Le **colonne** sono "domande" che l'AI risponde estraendo dal documento;
- Le **celle** sono le risposte generate.

Esempio: review "Compliance check" su 20 contratti, colonne =
"Data scadenza", "Penale recesso", "Foro competente". Per ogni contratto
l'AI compila le 3 celle.

### 7.2 Schema

`tabular_reviews` ([0002_tabular_workflow_hidden.sql:2-12](../migrations/0002_tabular_workflow_hidden.sql#L2-L12)):
```
id, user_id, project_id?, workflow_id?, title,
columns_config TEXT (JSON array), status, created_at, updated_at
```

`tabular_review_rows` ([0002_tabular_workflow_hidden.sql:14-22](../migrations/0002_tabular_workflow_hidden.sql#L14-L22)):
```
id, tabular_review_id (FK CASCADE), document_id?,
row_index INTEGER, cells TEXT (JSON array), status, created_at
```

### 7.3 Forma dei JSON

`columns_config`:
```json
[
  { "id": "col1", "name": "Data scadenza", "prompt": "Estrai la data di scadenza del contratto." },
  { "id": "col2", "name": "Penale recesso", "prompt": "Indica l'importo della penale per recesso anticipato." }
]
```

`cells` di una riga:
```json
[
  { "column_id": "col1", "value": "31/12/2026", "confidence": 0.94 },
  { "column_id": "col2", "value": "€ 5.000",  "confidence": 0.81 }
]
```

I formati esatti dipendono dall'implementazione frontend; il backend tratta
entrambi come stringhe JSON opache.

### 7.4 Endpoint

```
GET    /tabular-review?project_id={id?}     lista (filtro opzionale)
POST   /tabular-review                      body: { title?, project_id?, workflow_id?, columns_config? }
GET    /tabular-review/{id}                 dettaglio (columns_config parsato)
DELETE /tabular-review/{id}
```

### 7.5 Relazione con i Workflow

Il campo `workflow_id` (opzionale) lega la review a un workflow: si possono
costruire review **partendo da un workflow esistente** per riutilizzare lo
stesso prompt-base. La FK è `ON DELETE SET NULL` — eliminare il workflow
non distrugge le review.

### 7.6 UI

Pagina [tabular-reviews/page.tsx](../frontend/src/app/(pages)/tabular-reviews/page.tsx):
- Tab **All Reviews / In Project / Standalone**;
- Colonne mostrate: nome · n. colonne · n. documenti · progetto · data;
- Azioni: rename inline, delete (con guard owner), filter per progetto, multi-select.

---

## 8. Profilo utente e sicurezza

### 8.1 Profilo

```
GET    /user/profile                  → { id, username, display_name, created_at }
PUT    /user/profile                  body: { display_name? }
DELETE /user/account                  irreversibile, CASCADE su tutti i dati
```

### 8.2 PIN e biometrica

Vedi [src/routes/auth.rs](../src/routes/auth.rs):
- PIN 4–8 cifre, hashato Argon2id ([src/auth/pin.rs](../src/auth/pin.rs));
- Sessioni opaque token UUID v4 con TTL 1 settimana ([src/auth/session.rs](../src/auth/session.rs));
- Windows Hello via `IUserConsentVerifierInterop::RequestVerificationForWindowAsync` con HWND parent della finestra Tauri;
- Endpoint: `/auth/setup`, `/unlock`, `/unlock-biometric`, `/biometric-available`, `/biometric-enable`, `/biometric-disable`, `/change-pin`, `/status`, `/logout`.

### 8.3 LLM settings

Vedi sezione 2. Schema esteso da [migration 0003](../migrations/0003_user_settings_extend.sql).

```
GET /user/llm-settings    → tutti i campi (None se non configurati)
PUT /user/llm-settings    upsert; campi NULL nel body lasciano invariati i valori (logica COALESCE)
```

---

## 9. Riferimento rapido API

```
/auth/*            autenticazione (vedi 8.2)
/user/profile      profilo
/user/llm-settings provider LLM e modelli
/user/account      DELETE: hard reset

/project           progetti (CRUD)
/document          documenti (CRUD multipart)
/single-documents  alias di /document per il frontend Mike upstream
/chat              chat (POST root dispatcher: SSE se "messages" presente, altrimenti create record)
/chat/{id}/message    POST messaggio (legacy/deprecato in favore di POST /chat root)
/chat/{id}/messages   GET cronologia
/chat/{id}/generate-title  POST genera titolo
/workflow          workflow (CRUD)
/workflow/hidden   workflow nascosti per l'utente
/tabular-review    review tabulari (CRUD)
```

---

## 10. Flussi tipici

### Setup primo avvio
1. Lanciare l'app → schermata `/signup`.
2. Inserire username + PIN (4–8 cifre) + display name (opzionale).
3. Andare su **Account → Models**, configurare almeno **un provider** (più facile: locale via Ollama).
4. Salvare → tornare in **Assistant** e iniziare a chattare.

### Caricare un documento e chattare
1. **Assistant** → click su `+ Documents` → seleziona file (PDF / DOCX / XLSX / immagini / …).
2. Scrivi la domanda → invio. Il documento viene processato e iniettato nel system prompt.

### Creare una Tabular Review
1. **Tabular Review** → "New review".
2. Seleziona N documenti (idealmente da un progetto).
3. Definisci le colonne (nome + prompt per estrazione).
4. (Opzionale) lega un workflow.
5. Avvia la generazione: per ogni riga (= documento) l'AI compila le celle in base ai prompt delle colonne.

### Pulizia
- Cancellare un documento: eliminato dal DB e dallo storage filesystem.
- Cancellare un progetto: chat eliminate, documenti svincolati, review svincolate.
- "Lock / Sign Out" in Account: invalida la sessione (revoca tutti i token), il prossimo avvio chiederà di nuovo il PIN o Hello.

---

# Parte 2 — Guida operativa UI

> Cosa cliccare, dove guardare, in quale ordine. Le sezioni che seguono
> riproducono passo-passo l'esperienza utente nella finestra Tauri di MikeRust.

---

## 11. Layout globale e navigazione

### 11.1 Sidebar sinistra ([AppSidebar.tsx](../frontend/src/app/components/shared/AppSidebar.tsx))

Sempre visibile (toggle col tasto in alto a sinistra). Voci:

| Voce | Pagina | Cosa fa |
|---|---|---|
| **Assistant** | `/assistant` | Chat AI standalone, fuori da progetti |
| **Projects** | `/projects` | Cartelle di documenti + chat di progetto |
| **Tabular Review** | `/tabular-reviews` | Estrazione strutturata in tabella |
| **Workflows** | `/workflows` | Template di prompt riutilizzabili |
| **Account** | `/account` | Profilo, PIN, biometrica |

In fondo alla sidebar, il **chip profilo** mostra `username` + dot di stato.
Clic apre il menu: **Account Settings**, **Lock / Sign Out**.

Sotto il nodo Assistant, viene listata la **cronologia chat recenti**: ognuna
mostra il titolo (auto-generato dopo il primo messaggio) e il menu `…` con
**Rename** / **Delete** (entrambi disabilitati per chat di altri utenti — modal
"Owner-only action").

### 11.2 Finestra Tauri

Dimensione default 1280×800 (resizable, min 800×600), titolo "MikeRust".
Il dialog Windows Hello compare in primo piano grazie al fix
`IUserConsentVerifierInterop` con HWND parent (vedi sezione 8.2).

---

## 12. Primo avvio e accesso

### 12.1 Schermata di setup ([signup/page.tsx](../frontend/src/app/signup/page.tsx))

Apparirà solo la prima volta (DB vuoto):

| Campo | Validazione | Note |
|---|---|---|
| **Username** | non vuoto | Visibile in sidebar. Univoco. |
| **Display name** | opzionale | Es. "Avv. Mario Rossi" — usato in saluti. |
| **PIN** | 4–8 cifre | Hashato Argon2id, non recuperabile. |
| **Confirm PIN** | uguale al precedente | |

Click **Create profile** → setup chiama `POST /auth/setup`, riceve token, salva
in `localStorage.mike_auth_token`, redirect automatico a `/assistant`.

### 12.2 Schermata di login ([login/page.tsx](../frontend/src/app/login/page.tsx))

Comportamento adattivo basato su `/auth/biometric-available`:

- **Bio enrolled + hardware OK** → schermata "Windows Hello ready" con dialog
  auto-triggerato. Cancellando il dialog si torna alla schermata PIN.
- **PIN-only** → input numerico password-style, autofocus, pulsante **Unlock**.
  Sotto, link "Use Windows Hello" se enrolled.

Errori (`Wrong PIN`, `Biometric verification failed`) appaiono in box rosso
sopra il bottone.

---

## 13. Account ([account/page.tsx](../frontend/src/app/(pages)/account/page.tsx))

Pagina unica con quattro sezioni in colonna.

### 13.1 Profile

| Elemento | Comportamento |
|---|---|
| **Username** | Read-only (cambia solo da DB). |
| **Display Name** [input] + **Save** | `PUT /user/profile` con `{display_name}`. Bottone diventa "Saved ✓" per 2 secondi. |

### 13.2 Change PIN

Form: **Current PIN** / **New PIN (4–8)** / **Confirm New PIN**.
Validazione client: lunghezza min, mismatch confirm. Bottone **Update PIN**:
- ✓ verde "PIN updated successfully"
- ✗ rosso "Current PIN is incorrect" / "New PIN must be 4–8 digits"

### 13.3 Biometric (visibile solo se `available: true`)

| Stato | UI |
|---|---|
| `enabled: false` | Badge grigio "Disabled" + bottone **Enable Windows Hello** |
| `enabled: true`  | Badge verde "Enabled" + bottone **Disable Windows Hello** |

Premere **Enable** triggera il dialog OS — il fix HWND interop fa apparire
la finestra di Windows Hello in foreground sopra MikeRust.

Etichetta dinamica: "Touch ID" su macOS, "Windows Hello" altrove.

### 13.4 Actions

**Lock / Sign Out**: chiama `POST /auth/logout` (revoca tutte le sessioni
dell'utente lato server) + cancella `localStorage.mike_auth_token` e `mike_auth_user`.

---

## 14. Configurazione provider LLM ([account/models/page.tsx](../frontend/src/app/(pages)/account/models/page.tsx))

Pagina raggiungibile da **Account → Models** (link in cima alla pagina account).

### 14.1 Active Provider (in alto)

Quattro bottoni a tasti:
```
[ OpenAI / ChatGPT ] [ Anthropic Claude ]
[ Google Gemini    ] [ Local / OpenAI-compatible ]
```

Quello attivo è in nero, gli altri border-grey. Selezione = `activeProvider`.

### 14.2 Sezioni provider (sotto, una per provider)

Ogni sezione ha:
- Icona + titolo (es. "OpenAI / ChatGPT")
- **API Key** [password con toggle 👁 / 🚫]
- **Model** [input testo, placeholder es. "gpt-4o"]

Per **Local / OpenAI-compatible** in più:
- **Base URL** [input, default `http://localhost:11434/v1`]
- **API Key (leave empty for Ollama)** [opzionale]
- **Model name** [es. `gemma3:12b`, `llama3`, `mistral`]

### 14.3 Salvataggio

Pulsante **Save settings** in fondo:
1. Scrive in `localStorage.mikerust_llm_settings` (JSON).
2. Chiama `PUT /user/llm-settings` con i campi non-null mappati.
3. Mostra "Saved ✓" per 2 secondi.

> ⚠️ Senza salvare almeno un provider, la chat fallirà al primo invio
> ("API key not configured" o "Local model not configured").

---

## 15. Projects

### 15.1 Lista progetti ([projects/page.tsx](../frontend/src/app/(pages)/projects/page.tsx))

Tabella con colonne:
- **Name** (cliccabile → entra nel progetto)
- **Created** (data)
- Menu `…` per riga: **Rename**, **Delete**

In alto a destra: bottone **+ New project**.

### 15.2 Creazione progetto ([NewProjectModal.tsx](../frontend/src/app/components/projects/NewProjectModal.tsx))

Modale con:
- **Project name** [input testo, obbligatorio]
- **Description** [textarea, opzionale]
- (Sezioni **Members / Upload files** sono ereditate dal frontend Mike upstream
  e nel contesto local-single-user di MikeRust **non sono operative**: il
  backend ignora membership; gli upload funzionano ma vengono associati al
  progetto via `documents.project_id`.)

Click **Create project** → `POST /project` → redirect a `/projects/{id}`.

### 15.3 Dettaglio progetto ([ProjectPage.tsx](../frontend/src/app/components/projects/ProjectPage.tsx))

Tre tab nella vista progetto:

| Tab | Contenuto |
|---|---|
| **Documents** | Lista documenti del progetto. Drag-drop o `+ Add` per caricare. Per ogni file: download, rename, delete. |
| **Chats** | Conversazioni avviate dentro questo progetto (le chat ereditano i documenti come contesto). |
| **Tabular Reviews** | Review tabulari limitate a questo progetto. |

In testa al progetto: nome editabile inline, contatori file/chat/review.

---

## 16. Assistant (chat principale)

### 16.1 Vista iniziale ([InitialView.tsx](../frontend/src/app/components/assistant/InitialView.tsx))

Quando non ci sono messaggi: titolo "Hi, {Display Name}" + barra di input
centrata. Sotto la barra:

- **+ Documents** ([AddDocButton.tsx](../frontend/src/app/components/assistant/AddDocButton.tsx)):
  apre modale per allegare file
- **Workflows** (icona libreria): apre modale workflow
- **Modello selezionato** (combo a destra): mostra etichetta es. "Gemma 3 12B"
- **→** (freccia): invia (oppure Enter; Shift+Enter va a capo)

### 16.2 Combo modelli ([ModelToggle.tsx](../frontend/src/app/components/assistant/ModelToggle.tsx))

Dropdown raggruppato per **provider configurato**:

```
ANTHROPIC
  Claude Opus 4.7   ✓
  Claude Sonnet 4.6
GOOGLE
  Gemini 3.1 Pro
  Gemini 3 Flash
OPENAI
  gpt-4o
LOCAL
  gemma3:12b
```

I gruppi compaiono solo se la rispettiva API key è configurata
(o, per OpenAI/Local, se sono compilati `model` + credenziali).
Il modello selezionato è marcato `✓`. Modelli senza API key hanno icona
⚠ rossa e il messaggio "API key missing".

Auto-refresh quando cambiano le settings (storage event).

### 16.3 Allegare documenti ([AddDocumentsModal.tsx](../frontend/src/app/components/shared/AddDocumentsModal.tsx))

Apre un modale a schermo intero con due aree:

- **Drag & drop** (in alto) — accetta i formati estesi:
  `pdf, docx, doc, xlsx, xls, xlsb, ods, csv, txt, md, png, jpg, jpeg, tif, tiff`.
- **Lista esistenti** (in basso): documenti già caricati dell'utente, con
  checkbox multi-select.

Pulsante **Add** in fondo: chiude il modale e mostra i file come **chip blu**
sopra l'input bar. Ogni chip ha una × per rimuoverlo prima di inviare.

### 16.4 Selezionare un workflow ([AssistantWorkflowModal.tsx](../frontend/src/app/components/assistant/AssistantWorkflowModal.tsx))

Modale con la lista dei workflow dell'utente (esclusi quelli "hidden"):
ogni voce mostra `title` + preview del `prompt_md`. Click su uno → si attacca
come **chip viola** sopra l'input bar; il messaggio sarà inviato con il
`workflow.id` nel payload e il backend antepone il prompt-md al testo utente.

### 16.5 Vista chat in corso ([ChatView.tsx](../frontend/src/app/components/assistant/ChatView.tsx))

- Messaggi utente: bolla scura allineata a destra; eventuali allegati appaiono
  come chip sotto.
- Messaggi assistente: rendering Markdown (titoli, liste, code blocks, tabelle,
  citazioni). Spinner ✦ durante streaming. Indicatore "Thinking…" tra
  un'iterazione tool e la successiva (se modello usa tools).
- Pulsante **Stop** (square) sostituisce la freccia di invio durante lo
  streaming → cancella la richiesta lato client.

### 16.6 Errori visualizzati

Lo stream `data: {"type":"error","message":"…"}` dal backend produce un box
rosso sotto l'ultimo messaggio assistente (es. "Local LLM error 404 …, try
pulling it first"). Errori HTTP 4xx/5xx non-stream finiscono come testo
errore standard.

---

## 17. Workflows ([workflows/page.tsx](../frontend/src/app/(pages)/workflows/page.tsx))

### 17.1 Lista

Tabella di workflow con colonne **Title**, **Created**, e per riga: menu
`…` con **Edit**, **Delete**, **Hide** (aggiunge a `workflow_hidden`).

In alto: bottone **+ New workflow**.

### 17.2 Editor workflow

Form con:
- **Title** [input]
- **Prompt (Markdown)** [textarea grande, monospace]

Anteprima rendering markdown sotto la textarea. **Save** chiama `POST` o
`PUT /workflow/{id}`.

> Suggerimento: scrivi il prompt in seconda persona ("Riassumi il documento
> allegato in 5 punti…"); MikeRust lo concatena prima del messaggio utente
> nella chat che usa il workflow.

### 17.3 Workflow nascosti

Il toggle **Show hidden workflows** (in alto a destra della lista) mostra
anche quelli aggiunti a `workflow_hidden`, con icona `👁🚫`. Click su una
voce nascosta → menu `…` → **Unhide**.

---

## 18. Tabular Reviews

### 18.1 Overview ([tabular-reviews/page.tsx](../frontend/src/app/(pages)/tabular-reviews/page.tsx))

Pagina con tre tab:

| Tab | Cosa mostra |
|---|---|
| **All Reviews** | Tutte (di progetti + standalone). |
| **In Project** | Solo quelle con `project_id != null` (filtrabili per progetto in dropdown). |
| **Standalone** | Quelle senza progetto. |

Tabella con colonne **Name** (rename inline col doppio-click), **Columns**,
**Documents**, **Project**, **Created**, menu `…` (**Open**, **Delete**).

Multi-select (checkbox header) → bulk delete.

### 18.2 Creazione ([AddNewTRModal.tsx](../frontend/src/app/components/tabular/AddNewTRModal.tsx))

Modale guidata:
1. **Title** [input]
2. **Project** (opzionale, dropdown progetti) — se selezionato, la review
   eredita i documenti del progetto.
3. **Workflow** (opzionale, dropdown workflow) — se selezionato, ne usa il
   `prompt_md` come base per le colonne.
4. **Documents**: checkbox multi-select dai documenti dell'utente
   (filtrabili per progetto).
5. **Create Tabular Review** [bottone].

### 18.3 Editor ([TabularReviewView.tsx](../frontend/src/app/components/tabular/TabularReviewView.tsx))

Layout a tre pannelli:

```
┌─────────────────────────────────────────────────┐
│ TITLE [editabile]            [Generate] [Export]│
├──────────────────┬──────────────────────────────┤
│ Documents        │ Tabella                      │
│ ☐ contratto.pdf  │ │ Doc       │ Col1 │ Col2 │  │
│ ☐ statuto.docx   │ │ contratto│ ...  │ ...  │  │
│ + Add docs       │ │ statuto  │ ...  │ ...  │  │
│                  │ + Add column                  │
├──────────────────┴──────────────────────────────┤
│ TR Chat Panel (sidekick AI per analisi celle)   │
└─────────────────────────────────────────────────┘
```

### 18.4 Aggiungere colonne ([AddColumnModal.tsx](../frontend/src/app/components/tabular/AddColumnModal.tsx))

Modale:
- **Column name** [input] — header visibile della colonna
- **Type** [select: text / number / date / boolean]
- **Extraction prompt** [textarea] — il prompt che l'AI userà per ogni cella
  (es. *"Estrai la data di scadenza in formato gg/mm/aaaa"*)
- **Save column**

Le colonne sono persistite come elementi del JSON `columns_config`.

### 18.5 Generazione celle

Il bottone **Generate** in alto-destra avvia per ogni riga (= documento) un
ciclo che, per ogni colonna, manda al backend `POST /tabular-review/{id}/generate`
e popola le `cells` man mano che arrivano i risultati. Status per cella:
`pending` (puntini) → `complete` (testo) o `error` (icona ⚠).

### 18.6 Chat di review ([TRChatPanel.tsx](../frontend/src/app/components/tabular/TRChatPanel.tsx))

Pannello inferiore con un mini assistant: l'utente può chiedere "perché la
cella X dice questo?" o "rigenera questa cella con prompt diverso". Le
azioni di rigenerazione cella chiamano
`POST /tabular-review/{id}/regenerate-cell`.

### 18.7 Export

Bottone **Download** → genera Excel via la libreria client `exceljs`:
le righe diventano file scaricato, ogni colonna è un foglio. **Lato backend
non c'è endpoint export**: tutto avviene lato browser leggendo i `cells`
già caricati.

---

## 19. Modali condivise

| Modale | Uso |
|---|---|
| [OwnerOnlyModal.tsx](../frontend/src/app/components/shared/OwnerOnlyModal.tsx) | Mostrato quando l'utente prova a rinominare/eliminare risorse di altri utenti. Solo informativo, OK chiude. |
| [ApiKeyMissingModal.tsx](../frontend/src/app/components/shared/ApiKeyMissingModal.tsx) | Triggerato dal combo modelli quando si seleziona un provider senza API key. Linka direttamente a `/account/models`. |
| [AddDocumentsModal.tsx](../frontend/src/app/components/shared/AddDocumentsModal.tsx) | Multi-select documenti per chat / progetti / review. Drag-drop sopra. |
| [AddProjectDocsModal.tsx](../frontend/src/app/components/shared/AddProjectDocsModal.tsx) | Variante dedicata al detail di un progetto (filtra fuori i documenti già appartenenti). |
| [UploadNewVersionModal.tsx](../frontend/src/app/components/shared/UploadNewVersionModal.tsx) | Sostituire un documento mantenendo la stessa entità (versioning concettuale). |

---

## 20. Convenzioni di feedback visivo

| Stato | Pattern |
|---|---|
| Loading | Bottone disabilitato + testo "Saving…" / "Creating…" / "Updating…" / "Unlocking…" |
| Successo | Spunta ✓ verde "Saved" / "PIN updated successfully" per 2 secondi |
| Errore generico | Box rosso bg-red-50 con il messaggio del backend |
| Errore di provider LLM | Errore inline sotto l'ultimo messaggio assistant |
| Lista vuota | Testo placeholder grigio con CTA (es. "No models configured. Open Account → Models.") |
| Owner-only | Modale lock 🔒 (vedi 19) |

---

## 21. Riepilogo workflow operativo (cheat-sheet)

### A. Configurazione iniziale
1. Apri MikeRust → **Signup**: username, PIN, display name → **Create profile**.
2. Sidebar → **Account** → (opzionale) **Enable Windows Hello**.
3. **Account → Models**: seleziona Active provider, compila API Key + Model → **Save settings**.

### B. Caricare documenti e fare domande
1. Sidebar → **Assistant**.
2. Clic **+ Documents** → seleziona/uploada PDF/DOCX/XLSX/PNG/TIFF → **Add**.
3. Verifica modello in basso a destra (cambia se serve).
4. Scrivi domanda → **Enter**.

### C. Organizzare per pratica/cliente
1. **Projects → + New project**: nome + descrizione → **Create**.
2. Tab **Documents** del progetto: drag-drop o **+ Add**.
3. Tab **Chats**: chat con contesto del progetto.

### D. Estrazione dati strutturata
1. **Tabular Review → +** → titolo, (progetto opz.), seleziona N documenti → **Create**.
2. **+ Add column** per ogni dato che vuoi estrarre (nome + prompt).
3. **Generate** → l'AI compila le celle.
4. **Download** → Excel.

### E. Riusare prompt frequenti
1. **Workflows → + New workflow**: titolo + Markdown prompt → **Save**.
2. In Assistant: clic icona libreria → seleziona workflow → scrivi domanda.
3. Per nascondere temporaneamente un workflow: menu `…` → **Hide**.

### F. Manutenzione
- Cambiare PIN periodicamente → **Account → Change PIN**.
- Pulizia: cancellare singolo documento (Documents tab) o intero progetto (Projects).
- Reset totale: **Account → Lock / Sign Out** + manuale `DELETE /user/account` (irreversibile, CASCADE).
