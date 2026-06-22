# Distributing mike-legal (the on-device model)

mike-legal is a Qwen3-4B LoRA fine-tune, shipped as a ~2.5 GB quantized GGUF.
It is **never committed to git** (too large, and it is a derived artifact). It is
distributed through Ollama, which the app already talks to at `127.0.0.1:11434`.

Think of Ollama as an app store for local AI models: you upload the model once,
and anyone installs it with a single command.

## Publish it (you do this once)

The model is already built in your local Ollama (`ollama list` shows
`mike-legal`). To put it in the Ollama registry so others can pull it:

1. Make a free account at https://ollama.com and sign in. Your username there is
   your namespace (shown below as `YOUR_OLLAMA_USERNAME`).
2. Tag the local model under your namespace and push it:

   ```bash
   ollama cp mike-legal YOUR_OLLAMA_USERNAME/mike-legal
   ollama push YOUR_OLLAMA_USERNAME/mike-legal
   ```

That uploads the 2.5 GB once. Done.

## Install it (what your users run)

```bash
ollama pull YOUR_OLLAMA_USERNAME/mike-legal
```

One command, ~2.5 GB downloaded once, then cached. After that the app's
"mike-legal" option works fully offline. (The app can also do this for the user
automatically on first use; see the auto-pull note below.)

## Rebuild from the raw gguf (only if you ever lose the local model)

If `mike-legal` is gone from `ollama list`, recreate it from the quantized gguf:

```bash
# from the folder that has mike-legal-q4_K_M.gguf and this Modelfile
ollama create mike-legal -f Modelfile
```

## Notes

- The training **data** and the **gguf weights** never go to git. Only this
  Modelfile and guide do.
- The weights are open once downloaded, so the app shows a one-time PII
  disclosure before mike-legal can be used (ModelDataDisclosureGate). Consider
  putting the same notice on the model's Ollama page.
- Auto-pull (optional): the app can detect that mike-legal is not installed when
  the user selects it and pull it with a progress bar, so end users run no
  commands at all. Ask if you want this wired in.
