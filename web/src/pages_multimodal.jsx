// pages_multimodal.jsx — R-02/03/04/05 Multimodal control plane.
//
// Tabs: Vision / Image / Audio Transcribe / Audio Synth.
// Vision  → POST /api/v1/admin/multimodal/vision/analyze
// Image   → POST /api/v1/admin/multimodal/image/generate
// Trans.  → POST /api/v1/admin/multimodal/audio/transcribe (multipart)
// Synth   → POST /api/v1/admin/multimodal/audio/synthesize

const { useEffect: mmU, useState: mmS, useRef: mmR, useMemo: mmM } = React;
const {
  Card: MmCard, CardHeader: MmCardHeader, Button: MmButton, Input: MmInput,
  Textarea: MmTextarea, Tabs: MmTabs, Select: MmSelect, JsonViewer: MmJson,
  EmptyState: MmEmpty, Skeleton: MmSk, ErrorBanner: MmErr, Badge: MmBadge,
  RiskBadge: MmRiskBadge, Dialog: MmDialog, useToast: mmUseToast, I: MmI,
} = window.cc.ui || window;

const fileToB64 = (file) =>
  new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => {
      const s = String(r.result || '');
      const idx = s.indexOf(',');
      resolve(idx >= 0 ? s.slice(idx + 1) : s);
    };
    r.onerror = reject;
    r.readAsDataURL(file);
  });

// ---- Vision ---------------------------------------------------------------

const VisionPanel = ({ t }) => {
  const toast = mmUseToast();
  const fileRef = mmR(null);
  const [imageUrl, setImageUrl] = mmS('');
  const [imageB64, setImageB64] = mmS('');
  const [imagePreview, setImagePreview] = mmS('');
  const [prompt, setPrompt] = mmS('Describe this image in detail.');
  const [provider, setProvider] = mmS('');
  const [busy, setBusy] = mmS(false);
  const [result, setResult] = mmS(null);
  const [error, setError] = mmS(null);

  const onFile = async (e) => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    try {
      const b64 = await fileToB64(f);
      setImageB64(b64);
      setImageUrl('');
      setImagePreview(`data:${f.type || 'image/png'};base64,${b64}`);
    } catch (err) {
      toast && toast.toast({ title: t('multimodal.vision.upload_failed'), tone: 'error' });
    }
  };

  const onUrl = (v) => {
    setImageUrl(v);
    if (v) {
      setImageB64('');
      setImagePreview(v);
    } else {
      setImagePreview('');
    }
  };

  const reset = () => {
    setImageB64(''); setImageUrl(''); setImagePreview('');
    setResult(null); setError(null);
    if (fileRef.current) fileRef.current.value = '';
  };

  const run = async () => {
    if (!imageB64 && !imageUrl) {
      toast && toast.toast({ title: t('multimodal.vision.need_image'), tone: 'error' });
      return;
    }
    setBusy(true); setError(null); setResult(null);
    try {
      const start = performance.now();
      const resp = await window.cc.api.multimodal.vision({
        image_b64: imageB64 || undefined,
        image_url: imageUrl || undefined,
        prompt,
        provider: provider || undefined,
      });
      const ms = Math.round(performance.now() - start);
      setResult({ ...resp, _ms: ms });
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[460px_1fr] gap-4">
      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.vision.title')} subtitle={<span className="mono">POST /api/v1/admin/multimodal/vision/analyze</span>} />
        <div className="p-4 space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.vision.image')}</div>
            <div className="flex items-center gap-2">
              <input ref={fileRef} type="file" accept="image/*" onChange={onFile} className="hidden" id="vision-upload" />
              <label htmlFor="vision-upload"
                className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-[var(--border-strong)] bg-[var(--bg-2)] text-[12px] text-fg-2 hover:text-fg cursor-pointer">
                <MmI.Folder size={12} /> {t('multimodal.vision.choose_file')}
              </label>
              <span className="text-[11px] text-fg-4 mono">{t('multimodal.vision.or')}</span>
              <MmInput
                value={imageUrl}
                onChange={(e) => onUrl(e.target.value)}
                placeholder="https://…"
                className="flex-1"
              />
            </div>
            {imagePreview && (
              <div className="mt-2 border border-border rounded-md overflow-hidden bg-bg-3 max-h-[260px] flex items-center justify-center">
                <img src={imagePreview} alt="preview" className="max-h-[260px]" />
              </div>
            )}
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.prompt')}</div>
            <MmTextarea rows={3} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.provider')}</div>
            <MmInput value={provider} onChange={(e) => setProvider(e.target.value)} placeholder={t('multimodal.provider.auto')} />
          </div>
          <div className="flex justify-between items-center pt-2 border-t border-border/60">
            <MmButton variant="ghost" onClick={reset}>{t('multimodal.reset')}</MmButton>
            <MmButton onClick={run} disabled={busy}>
              {busy ? <><MmI.Loader size={12} className="animate-spin" /> {t('multimodal.analyzing')}</>
                    : <><MmI.Eye size={12} /> {t('multimodal.vision.analyze')}</>}
            </MmButton>
          </div>
        </div>
      </MmCard>

      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.result')}
          subtitle={result && result._ms ? <span className="mono">{result._ms} ms · {result.provider || '—'}</span> : null} />
        <div className="p-4 space-y-3">
          {error && <MmErr message={error} />}
          {!result && !error && <MmEmpty icon={MmI.Eye} title={t('multimodal.vision.empty.title')} subtitle={t('multimodal.vision.empty.subtitle')} />}
          {result && (
            <>
              <div className="bg-bg-3 border border-border rounded-md p-3 text-[13px] text-fg whitespace-pre-wrap leading-relaxed">
                {result.text || result.output || t('multimodal.no_text')}
              </div>
              <details className="text-[11px] text-fg-3">
                <summary className="cursor-pointer hover:text-fg">{t('multimodal.raw_json')}</summary>
                <div className="mt-2"><MmJson value={result} /></div>
              </details>
            </>
          )}
        </div>
      </MmCard>
    </div>
  );
};

// ---- Image ---------------------------------------------------------------

const IMAGE_SIZES = [
  { value: '512x512',   label: '512×512' },
  { value: '1024x1024', label: '1024×1024 (default)' },
  { value: '1024x1792', label: '1024×1792 (portrait)' },
  { value: '1792x1024', label: '1792×1024 (landscape)' },
  { value: '2048x2048', label: '2048×2048 (large)' },
];

const ImagePanel = ({ t }) => {
  const toast = mmUseToast();
  const [prompt, setPrompt] = mmS('A neon-lit Tokyo alley at midnight, cinematic.');
  const [size, setSize] = mmS('1024x1024');
  const [provider, setProvider] = mmS('');
  const [busy, setBusy] = mmS(false);
  const [result, setResult] = mmS(null);
  const [error, setError] = mmS(null);
  const [confirm, setConfirm] = mmS(false);

  const isLarge = size === '2048x2048';

  const doRun = async () => {
    setBusy(true); setError(null); setResult(null);
    const start = performance.now();
    try {
      const resp = await window.cc.api.multimodal.imageGenerate({
        prompt, size, provider: provider || undefined,
      });
      const ms = Math.round(performance.now() - start);
      setResult({ ...resp, _ms: ms });
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const onSubmit = () => {
    if (!prompt.trim()) {
      toast && toast.toast({ title: t('multimodal.image.need_prompt'), tone: 'error' });
      return;
    }
    if (isLarge) { setConfirm(true); return; }
    doRun();
  };

  const imgB64 = result && (result.image_b64 || result.b64);
  const imgUrl = result && result.image_url;
  const imgMime = (result && result.mime) || 'image/png';

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[460px_1fr] gap-4">
      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.image.title')}
          subtitle={<span className="mono">POST /api/v1/admin/multimodal/image/generate</span>}
          right={isLarge ? <MmRiskBadge level="high" /> : null} />
        <div className="p-4 space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.prompt')}</div>
            <MmTextarea rows={5} value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.image.size')}</div>
              <MmSelect value={size} onChange={setSize} options={IMAGE_SIZES} />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.provider')}</div>
              <MmInput value={provider} onChange={(e) => setProvider(e.target.value)} placeholder={t('multimodal.provider.auto')} />
            </div>
          </div>
          <MmButton onClick={onSubmit} disabled={busy}>
            {busy ? <><MmI.Loader size={12} className="animate-spin" /> {t('multimodal.image.generating')}</>
                  : <><MmI.Sparkles size={12} /> {t('multimodal.image.generate')}</>}
          </MmButton>
        </div>
      </MmCard>

      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.result')}
          subtitle={result && result._ms ? <span className="mono">{result._ms} ms · {result.provider || '—'}</span> : null} />
        <div className="p-4 space-y-3">
          {error && <MmErr message={error} />}
          {!result && !error && !busy && (
            <MmEmpty icon={MmI.Sparkles} title={t('multimodal.image.empty.title')} subtitle={t('multimodal.image.empty.subtitle')} />
          )}
          {busy && (
            <div className="border border-dashed border-border rounded-md p-12 text-center text-[12px] text-fg-3 mono">
              <MmI.Loader size={20} className="animate-spin mx-auto mb-2" />
              {t('multimodal.image.generating')}
            </div>
          )}
          {(imgB64 || imgUrl) && (
            <div>
              <div className="border border-border rounded-md overflow-hidden bg-bg-3 flex items-center justify-center">
                <img src={imgB64 ? `data:${imgMime};base64,${imgB64}` : imgUrl} alt="generated"
                     className="max-w-full max-h-[600px] block" />
              </div>
              {result && result.revised_prompt && (
                <div className="mt-3">
                  <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1">{t('multimodal.image.revised_prompt')}</div>
                  <div className="bg-bg-3 border border-border rounded-md p-2.5 text-[12px] text-fg-2 italic">
                    {result.revised_prompt}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </MmCard>

      <MmDialog
        open={confirm}
        onClose={() => setConfirm(false)}
        title={t('multimodal.image.confirm_large.title')}
        subtitle={t('multimodal.image.confirm_large.subtitle')}
        footer={<>
          <MmButton variant="ghost" onClick={() => setConfirm(false)}>{t('multimodal.cancel')}</MmButton>
          <MmButton onClick={() => { setConfirm(false); doRun(); }}>
            <MmI.Sparkles size={12} /> {t('multimodal.image.confirm_large.go')}
          </MmButton>
        </>}>
        <p className="text-[13px] text-fg-2">{t('multimodal.image.confirm_large.body')}</p>
      </MmDialog>
    </div>
  );
};

// ---- Audio Transcribe ----------------------------------------------------

const TranscribePanel = ({ t }) => {
  const toast = mmUseToast();
  const fileRef = mmR(null);
  const [file, setFile] = mmS(null);
  const [busy, setBusy] = mmS(false);
  const [result, setResult] = mmS(null);
  const [error, setError] = mmS(null);

  const onFile = (e) => {
    const f = e.target.files && e.target.files[0];
    if (f) { setFile(f); setResult(null); setError(null); }
  };

  const run = async () => {
    if (!file) {
      toast && toast.toast({ title: t('multimodal.transcribe.need_file'), tone: 'error' });
      return;
    }
    setBusy(true); setError(null); setResult(null);
    try {
      const start = performance.now();
      const resp = await window.cc.api.multimodal.audioTranscribe(file);
      const ms = Math.round(performance.now() - start);
      setResult({ ...resp, _ms: ms });
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const formatTs = (s) => {
    const total = Math.round(Number(s) || 0);
    const m = Math.floor(total / 60);
    const sec = total % 60;
    return `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`;
  };

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[420px_1fr] gap-4">
      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.transcribe.title')}
          subtitle={<span className="mono">POST /api/v1/admin/multimodal/audio/transcribe</span>} />
        <div className="p-4 space-y-3">
          <input ref={fileRef} type="file" accept="audio/*,video/*" onChange={onFile}
                 id="trans-upload" className="hidden" />
          <label htmlFor="trans-upload"
            className="block border border-dashed border-border rounded-md py-8 text-center cursor-pointer hover:border-[var(--accent-ring)] hover:bg-accent-soft/30 transition-colors">
            <MmI.Mic size={22} className="mx-auto text-fg-3 mb-2" />
            <div className="text-[12.5px] text-fg-2">
              {file ? file.name : t('multimodal.transcribe.choose')}
            </div>
            {file && (
              <div className="text-[11px] mono text-fg-4 mt-1">
                {(file.size / 1024).toFixed(1)} KB · {file.type || 'audio'}
              </div>
            )}
          </label>
          <MmButton onClick={run} disabled={busy || !file}>
            {busy ? <><MmI.Loader size={12} className="animate-spin" /> {t('multimodal.transcribe.processing')}</>
                  : <><MmI.Play size={12} /> {t('multimodal.transcribe.run')}</>}
          </MmButton>
        </div>
      </MmCard>

      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.result')}
          subtitle={result && result._ms ? <span className="mono">{result._ms} ms · {result.language || '—'}</span> : null} />
        <div className="p-4 space-y-3">
          {error && <MmErr message={error} />}
          {!result && !error && <MmEmpty icon={MmI.Mic} title={t('multimodal.transcribe.empty.title')} subtitle={t('multimodal.transcribe.empty.subtitle')} />}
          {result && (
            <>
              <div>
                <div className="flex items-center gap-2 text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
                  <span>{t('multimodal.transcribe.text')}</span>
                  {result.language && <MmBadge tone="cyan">{result.language}</MmBadge>}
                </div>
                <div className="bg-bg-3 border border-border rounded-md p-3 text-[13px] text-fg whitespace-pre-wrap leading-relaxed">
                  {result.text || t('multimodal.no_text')}
                </div>
              </div>
              {Array.isArray(result.segments) && result.segments.length > 0 && (
                <div>
                  <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
                    {t('multimodal.transcribe.segments')} ({result.segments.length})
                  </div>
                  <div className="border border-border rounded-md overflow-hidden divide-y divide-border max-h-[280px] overflow-y-auto">
                    {result.segments.map((seg, i) => (
                      <div key={i} className="px-3 py-2 grid grid-cols-[80px_1fr] gap-3 text-[12px]">
                        <div className="mono text-[11px] text-fg-4">
                          {formatTs(seg.start)} → {formatTs(seg.end)}
                        </div>
                        <div className="text-fg-2 leading-relaxed">{seg.text}</div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      </MmCard>
    </div>
  );
};

// ---- Audio Synth ---------------------------------------------------------

const SYNTH_VOICES = [
  { value: 'alloy',   label: 'alloy' },
  { value: 'echo',    label: 'echo' },
  { value: 'fable',   label: 'fable' },
  { value: 'onyx',    label: 'onyx' },
  { value: 'nova',    label: 'nova' },
  { value: 'shimmer', label: 'shimmer' },
];

const SynthPanel = ({ t }) => {
  const toast = mmUseToast();
  const [text, setText] = mmS('Welcome to CyberClaw. The console is online.');
  const [voice, setVoice] = mmS('alloy');
  const [provider, setProvider] = mmS('');
  const [busy, setBusy] = mmS(false);
  const [audioUrl, setAudioUrl] = mmS(null);
  const [result, setResult] = mmS(null);
  const [error, setError] = mmS(null);

  // Cleanup blob URL on change/unmount
  mmU(() => () => { if (audioUrl) try { URL.revokeObjectURL(audioUrl); } catch {} }, [audioUrl]);

  const run = async () => {
    if (!text.trim()) {
      toast && toast.toast({ title: t('multimodal.synth.need_text'), tone: 'error' });
      return;
    }
    setBusy(true); setError(null); setResult(null);
    if (audioUrl) { try { URL.revokeObjectURL(audioUrl); } catch {} setAudioUrl(null); }
    try {
      const start = performance.now();
      const resp = await window.cc.api.multimodal.audioSynthesize({
        text, voice, provider: provider || undefined,
      });
      const ms = Math.round(performance.now() - start);
      setResult({ ...resp, _ms: ms });
      const b64 = resp.audio_b64 || resp.b64;
      const mime = resp.mime || 'audio/mpeg';
      if (b64) {
        const bin = atob(b64);
        const arr = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
        const blob = new Blob([arr], { type: mime });
        setAudioUrl(URL.createObjectURL(blob));
      }
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[460px_1fr] gap-4">
      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.synth.title')}
          subtitle={<span className="mono">POST /api/v1/admin/multimodal/audio/synthesize</span>} />
        <div className="p-4 space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.synth.text')}</div>
            <MmTextarea rows={6} value={text} onChange={(e) => setText(e.target.value)} />
            <div className="text-[10px] mono text-fg-4 mt-1 text-right">{text.length} chars</div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.synth.voice')}</div>
              <MmSelect value={voice} onChange={setVoice} options={SYNTH_VOICES} />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('multimodal.provider')}</div>
              <MmInput value={provider} onChange={(e) => setProvider(e.target.value)} placeholder={t('multimodal.provider.auto')} />
            </div>
          </div>
          <MmButton onClick={run} disabled={busy}>
            {busy ? <><MmI.Loader size={12} className="animate-spin" /> {t('multimodal.synth.synthesizing')}</>
                  : <><MmI.Radio size={12} /> {t('multimodal.synth.synthesize')}</>}
          </MmButton>
        </div>
      </MmCard>

      <MmCard className="overflow-hidden">
        <MmCardHeader title={t('multimodal.result')}
          subtitle={result && result._ms ? <span className="mono">{result._ms} ms · {result.provider || '—'}</span> : null} />
        <div className="p-4 space-y-3">
          {error && <MmErr message={error} />}
          {!audioUrl && !error && (
            <MmEmpty icon={MmI.Radio} title={t('multimodal.synth.empty.title')} subtitle={t('multimodal.synth.empty.subtitle')} />
          )}
          {audioUrl && (
            <div className="bg-bg-3 border border-border rounded-md p-4 space-y-3">
              <audio controls src={audioUrl} className="w-full" />
              <div className="flex items-center gap-2 justify-end">
                <a href={audioUrl} download={`tts-${voice}.mp3`}
                   className="inline-flex items-center gap-1 text-[12px] text-accent hover:underline">
                  <MmI.Download size={11} /> {t('multimodal.synth.download')}
                </a>
              </div>
            </div>
          )}
        </div>
      </MmCard>
    </div>
  );
};

// ---- Page ----------------------------------------------------------------

const MultimodalPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = mmS('vision');
  return (
    <div className="space-y-4">
      <MmTabs value={tab} onChange={setTab} items={[
        { value: 'vision',     label: t('multimodal.tab.vision') },
        { value: 'image',      label: t('multimodal.tab.image') },
        { value: 'transcribe', label: t('multimodal.tab.transcribe') },
        { value: 'synth',      label: t('multimodal.tab.synth') },
      ]} />
      {tab === 'vision'     && <VisionPanel t={t} />}
      {tab === 'image'      && <ImagePanel t={t} />}
      {tab === 'transcribe' && <TranscribePanel t={t} />}
      {tab === 'synth'      && <SynthPanel t={t} />}
    </div>
  );
};

Object.assign(window, { MultimodalPage });
