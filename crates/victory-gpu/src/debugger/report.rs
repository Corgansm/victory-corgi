//! The recorded frame's report: one HTML page (`frame-<n>.html`, and
//! `latest.html`) in the debugger's folder, beside the frame's listing and
//! the pictures of its targets, which it links. Everything it shows is in
//! the page as data; its script lays it out.

use std::fmt::Write as _;
use std::path::PathBuf;

use super::pixel::{Sample, decode};
use super::textures::Texture;
use super::trace::Trace;
use super::{Request, json};

pub(super) fn write(
    frame: u64,
    request: &Request,
    pixels: &[Sample],
    traces: &[Trace],
    textures: &[Texture],
) -> std::io::Result<PathBuf> {
    let dir = super::dir().ok_or_else(|| std::io::Error::other("no debugger folder"))?;
    let capture = crate::capture::folder().cloned().unwrap_or_else(|| dir.clone());
    let ops_name = format!("frame-{frame}-ops.txt");
    // the recording's frames one after the other (`super::SPAN`)
    let mut ops = String::new();
    for n in frame..frame + super::SPAN {
        if let Ok(text) = std::fs::read_to_string(capture.join(format!("frame-{n}-ops.txt"))) {
            ops.push_str(&format!("0 list 0x0 ---- frame {n} ----\n"));
            ops.push_str(&text);
        }
    }
    // the pictures of the frame's targets (`crate::capture::dump_targets`)
    let mut pictures: Vec<String> = std::fs::read_dir(&capture)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| {
                    (frame..frame + super::SPAN).any(|f| n.starts_with(&format!("frame-{f}-"))) && n.ends_with(".png")
                })
                .collect()
        })
        .unwrap_or_default();
    pictures.sort();
    let prefix = if capture == *dir { String::new() } else { format!("file:///{}/", capture.display()).replace('\\', "/") };
    let mut data = String::new();
    let _ = writeln!(data, "const FRAME = {frame};");
    let _ = writeln!(data, "const REQUEST = {};", json(&format!("{:?}", request)));
    let _ = writeln!(data, "const PREFIX = {};", json(&prefix));
    let _ = writeln!(data, "const OPS_FILE = {};", json(&ops_name));
    data.push_str("const OPS = [\n");
    for line in ops.lines() {
        let _ = writeln!(data, "{},", json(line));
    }
    data.push_str("];\nconst PICTURES = [");
    for p in &pictures {
        let _ = write!(data, "{},", json(p));
    }
    data.push_str("];\nconst PIXELS = [\n");
    for s in pixels {
        let values: Vec<String> = decode(s.format, &s.bytes).iter().map(|v| format!("{v:?}")).collect();
        let hex: String = s.bytes.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(
            data,
            "{{op:{},label:{},after:{},target:\"{:#x}\",format:{},w:{},h:{},x:{},y:{},v:[{}],hex:\"{}\"}},",
            s.op,
            json(&s.label),
            s.after,
            s.target,
            s.format,
            s.size.0,
            s.size.1,
            s.texel.0,
            s.texel.1,
            values.join(",").replace("NaN", "null").replace("inf", "1e999"),
            hex
        );
    }
    data.push_str("];\nconst TEXTURES = [\n");
    for t in textures {
        let _ = writeln!(
            data,
            "{{id:\"{:#x}\",addr:\"{:#x}\",kind:\"{}\",w:{},h:{},d:{},mips:{},format:{},flags:{},host:{},mb:{:.2},fills:{},filled:{},refused:{},gone:{},placed:{},ago:{},preview:{}}},",
            t.this,
            t.address,
            t.kind,
            t.size.0,
            t.size.1,
            t.size.2,
            t.mips,
            t.format,
            t.flags,
            t.host,
            t.host_bytes as f64 / 1048576.0,
            t.title_fills,
            t.filled,
            t.refused,
            t.gone,
            t.placed,
            if t.frames_ago == u64::MAX { -1 } else { t.frames_ago as i64 },
            t.preview.as_deref().map_or("null".to_owned(), json)
        );
    }
    data.push_str("];\nconst TRACES = [\n");
    for t in traces {
        let _ = write!(
            data,
            "{{tag:{},hash:{},stage:{},lane:[{},{}],steps:[",
            t.tag,
            json(&t.hash),
            json(t.stage),
            t.lane.0,
            t.lane.1
        );
        for s in &t.steps {
            let _ = write!(data, "[{},{},{},{},{}],", s.order, s.offset, json(&s.text), json(&s.register), s.value);
        }
        data.push_str("],code:[");
        for (o, text) in &t.code {
            let _ = write!(data, "[{o},{}],", json(text));
        }
        data.push_str("]},\n");
    }
    data.push_str("];\n");
    let page = PAGE.replace("/*DATA*/", &data);
    let path = dir.join(format!("frame-{frame}.html"));
    std::fs::write(&path, &page)?;
    let _ = std::fs::write(dir.join("latest.html"), &page);
    Ok(path)
}

const PAGE: &str = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>VIctory frame report</title>
<style>
:root{--bg:#101418;--panel:#171d23;--line:#26303a;--text:#d7e0e8;--dim:#8b98a5;--hi:#64ff78;--warn:#ffb454;--bad:#ff6b6b;--acc:#59b7ff}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:13px/1.45 system-ui,Segoe UI,sans-serif}
header{padding:14px 18px;border-bottom:1px solid var(--line);display:flex;gap:24px;align-items:baseline;flex-wrap:wrap}
header h1{font-size:17px;margin:0}header .m{color:var(--dim)}
nav{display:flex;gap:4px;padding:8px 18px;border-bottom:1px solid var(--line);position:sticky;top:0;background:var(--bg);z-index:2}
nav button{background:var(--panel);color:var(--text);border:1px solid var(--line);padding:6px 12px;border-radius:6px;cursor:pointer}
nav button.on{border-color:var(--acc);color:var(--acc)}
section{display:none;padding:12px 18px}section.on{display:block}
input,select{background:var(--panel);color:var(--text);border:1px solid var(--line);border-radius:5px;padding:5px 8px}
table{border-collapse:collapse;width:100%;margin-top:8px}th,td{border-bottom:1px solid var(--line);padding:4px 6px;text-align:left;vertical-align:top}
th{position:sticky;top:44px;background:var(--panel);cursor:pointer;font-weight:600}
td.mono,.mono{font-family:Consolas,monospace;font-size:12px}
tr.pass td{background:#1d2630;color:var(--acc);font-weight:600}
tr.changed td{background:#1c2a1d}tr.drop td{color:var(--bad)}
.sw{display:inline-block;width:14px;height:14px;border:1px solid #000;vertical-align:middle;margin-right:4px}
.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(260px,1fr));gap:12px}
.grid figure{margin:0;background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:6px}
.grid img{width:100%;image-rendering:pixelated}.grid figcaption{font-size:11px;color:var(--dim);word-break:break-all}
.detail{white-space:pre-wrap;word-break:break-all;font-family:Consolas,monospace;font-size:12px;color:var(--dim);display:none}
tr.open .detail{display:block}.tag{display:inline-block;padding:0 6px;border-radius:9px;font-size:11px;margin-right:3px}
.ok{background:#1f3a24;color:var(--hi)}.no{background:#3a1f1f;color:var(--bad)}.mid{background:#3a321f;color:var(--warn)}
img.th{width:48px;height:48px;object-fit:contain;image-rendering:pixelated;background:#000}
.note{color:var(--dim);max-width:900px}
</style></head><body>
<header><h1>VIctory frame <span id="fr"></span></h1><span class="m" id="sum"></span></header>
<nav id="tabs"></nav>
<section id="ops"><input id="opf" placeholder="filter: shader hash, pipeline, target, kind…" size="60"> <label><input type="checkbox" id="opdrop"> drops only</label>
<table><thead><tr><th>#</th><th>list</th><th>what</th><th>pipeline / shaders</th><th>targets</th></tr></thead><tbody id="opb"></tbody></table></section>
<section id="pix"><p class="note">The picked pixel, read from each texture an operation writes, before and after it. Rows where the value changed are green: the last green row of the presented target is what the screen shows.</p>
<label><input type="checkbox" id="pxch" checked> changed only</label> <input id="pxf" placeholder="filter texture or label">
<table><thead><tr><th>op</th><th>operation</th><th>texture</th><th>texel</th><th>before</th><th>after</th></tr></thead><tbody id="pxb"></tbody></table></section>
<section id="tex"><p class="note">Every title texture alive. "host" = the card has its copy; "filled" = an upload reached that copy (a texture the title fills that is not filled shows the neutral grey fallback); "refused" = the card would not make it.</p>
<input id="txf" placeholder="filter id / format / size"> <select id="txs"><option value="">all</option><option value="nohost">no host copy</option><option value="unfilled">title fills, not filled</option><option value="refused">refused</option><option value="recent">used in the last 10 frames</option></select>
<table><thead><tr><th data-k="preview">pic</th><th data-k="id">texture</th><th data-k="addr">address</th><th data-k="kind">dim</th><th data-k="w">size</th><th data-k="mips">mips</th><th data-k="format">format</th><th data-k="host">state</th><th data-k="mb">MB</th><th data-k="ago">frames ago</th></tr></thead><tbody id="txb"></tbody></table></section>
<section id="pipes"><p class="note">Every pipeline the recorded frames ran: its shaders, how many operations used it, and its whole graphics state (depth, stencil, rasterizer, topology, samples, each target's format and blend, the input layout). Click a row for the targets it drew into.</p>
<input id="ppf" placeholder="filter shader hash / state…" size="60">
<table><thead><tr><th>ops</th><th>pipeline / shaders</th><th>state</th></tr></thead><tbody id="ppb"></tbody></table></section>
<section id="trc"><p class="note">Shaders named in VICTORY_SHADER_TRACE, for the picked pixel (pixel shaders) or thread (compute): each console instruction the lane ran and the registers it wrote, in order.</p><div id="trb"></div></section>
<section id="tgt"><div class="grid" id="tgb"></div></section>
<script>
/*DATA*/
const $=id=>document.getElementById(id);
const esc=s=>String(s).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
$('fr').textContent=FRAME;
$('sum').textContent=`${OPS.length} operations · ${PICTURES.length} target pictures · ${PIXELS.length} pixel reads · ${TEXTURES.length} textures · ${TRACES.length} shader traces · ${REQUEST}`;
const tabs=[['ops','Operations'],['pix','Pixel history'],['pipes','Pipelines'],['tex','Textures'],['trc','Shader traces'],['tgt','Targets']];
for(const [id,name] of tabs){const b=document.createElement('button');b.textContent=name;b.onclick=()=>{for(const [i] of tabs){$(i).classList.toggle('on',i==id);}for(const x of $('tabs').children)x.classList.toggle('on',x==b);};$('tabs').append(b);}
$('tabs').children[PIXELS.length?1:0].click();
// operations
function parseOp(line){const m=line.match(/^(\d+) list (0x[0-9a-f]+) (.*)$/);if(!m)return{n:'',list:'',rest:line};
 const rest=m[3];const kind=rest.split(' ')[0];const pipe=(rest.match(/(?:compute|vertex|pixel|mesh)[^|]*?(?= targets| \||$)/)||[''])[0];
 const tg=(rest.match(/targets (\[[^\]]*\])/)||['',''])[1];const op=(rest.match(/\[op (\d+)\]/)||['',''])[1];const tag=(rest.match(/\[tag (\d+)\]/)||['',''])[1];
 const shaders=[...rest.matchAll(/\| ([0-9a-f]{16}):/g)].map(x=>x[1]);
 return{n:m[1],list:m[2],kind,rest,pipe,tg,shaders,op,tag,drop:/drop/i.test(rest)};}
const OPP=OPS.map(parseOp);
const BYOP={};for(const o of OPP){if(o.op)BYOP[o.op]=o;}
const stateOf=o=>(o.rest.match(/state \[(.*?)\] (?:scissor|viewport|\|)/)||o.rest.match(/state \[(.*?)\]/)||['',''])[1];
function drawOps(){const f=$('opf').value.toLowerCase();const d=$('opdrop').checked;let last='';const rows=[];
 for(const o of OPP){if(f&&!o.rest.toLowerCase().includes(f))continue;if(d&&!o.drop)continue;
  if(o.tg&&o.tg!=last){rows.push(`<tr class="pass"><td colspan="5">pass → ${esc(o.tg)}</td></tr>`);last=o.tg;}
  rows.push(`<tr class="${o.drop?'drop':''}" onclick="this.classList.toggle('open')"><td class="mono">${o.n}${o.op?'<br>op '+o.op:''}${o.tag?'<br>tag '+o.tag:''}</td><td class="mono">${esc(o.list)}</td><td>${esc(o.kind)}</td><td class="mono">${esc(o.pipe)} ${o.shaders.map(s=>'<span class="tag mid">'+s+'</span>').join('')}<div class="detail">${esc(o.rest)}</div></td><td class="mono">${esc(o.tg)}</td></tr>`);
  if(rows.length>6000){rows.push('<tr><td colspan=5>… narrow the filter</td></tr>');break;}}
 $('opb').innerHTML=rows.join('');}
$('opf').oninput=drawOps;$('opdrop').onchange=drawOps;drawOps();
// pixel history
const fmtv=v=>v.map(x=>x===null?'NaN':(Math.abs(x)>=1e5||(Math.abs(x)<1e-3&&x!=0)?x.toExponential(3):(+x.toFixed(4)))).join(', ');
const sw=v=>{if(!v.length)return'';const c=v.slice(0,3).map(x=>Math.round(Math.max(0,Math.min(1,(x===null?0:x)))*255));while(c.length<3)c.push(c[0]);return `<span class="sw" style="background:rgb(${c})"></span>`;};
function drawPix(){const ch=$('pxch').checked,f=$('pxf').value.toLowerCase();const by={};
 for(const s of PIXELS){const k=s.op+'/'+s.target;(by[k]=by[k]||{op:s.op,label:s.label,target:s.target,s})[s.after?'a':'b']=s;}
 const rows=[];for(const r of Object.values(by).sort((x,y)=>x.op-y.op)){const b=r.b,a=r.a;const changed=a&&b&&a.hex!=b.hex;
  const lone=b&&!a;if(ch&&!changed&&!lone)continue;if(f&&!(r.label+r.target).toLowerCase().includes(f))continue;
  const o=BYOP[r.op];const det=o?`<div class="detail">${esc(stateOf(o))}\n\n${esc(o.rest)}</div>`:'';
  rows.push(`<tr class="${changed?'changed':''}" onclick="this.classList.toggle('open')"><td class="mono">${r.op}</td><td class="mono">${esc(r.label)}${lone?' <span class="tag mid">depth before the draw</span>':''}${det}</td><td class="mono">${r.target}<br>${r.s.w}x${r.s.h} f${r.s.format}</td><td class="mono">${r.s.x},${r.s.y}</td><td class="mono">${b?sw(b.v)+fmtv(b.v)+'<br>'+b.hex:''}</td><td class="mono">${a?sw(a.v)+fmtv(a.v)+'<br>'+a.hex:''}</td></tr>`);}
 $('pxb').innerHTML=rows.join('')||'<tr><td colspan=6>No pixel reads: press F9 with the mouse over the game.</td></tr>';}
$('pxch').onchange=drawPix;$('pxf').oninput=drawPix;drawPix();
// pipelines
const PIPES={};for(const o of OPP){if(!o.pipe)continue;const k=o.pipe.trim();const p=(PIPES[k]=PIPES[k]||{pipe:k,n:0,state:'',targets:new Set()});p.n++;const st=stateOf(o);if(st)p.state=st;if(o.tg)p.targets.add(o.tg);}
function drawPipes(){const f=$('ppf').value.toLowerCase();const rows=[];
 for(const p of Object.values(PIPES).sort((a,b)=>b.n-a.n)){if(f&&!(p.pipe+p.state).toLowerCase().includes(f))continue;
  rows.push(`<tr onclick="this.classList.toggle('open')"><td class="mono">${p.n}</td><td class="mono">${esc(p.pipe)}<div class="detail">${esc([...p.targets].join('\n'))}</div></td><td class="mono">${esc(p.state).replace(/; /g,'<br>')}</td></tr>`);
  if(rows.length>3000)break;}
 $('ppb').innerHTML=rows.join('');}
$('ppf').oninput=drawPipes;drawPipes();
// textures
let txk='ago',txd=1;
function drawTex(){const f=$('txf').value.toLowerCase(),s=$('txs').value;let l=TEXTURES.filter(t=>{
 if(s=='nohost'&&t.host)return false;if(s=='unfilled'&&!(t.fills&&t.host&&!t.filled))return false;if(s=='refused'&&!t.refused)return false;if(s=='recent'&&!(t.ago>=0&&t.ago<=10))return false;
 if(f&&!(t.id+' '+t.addr+' f'+t.format+' '+t.w+'x'+t.h).toLowerCase().includes(f))return false;return true;});
 l.sort((x,y)=>{let a=x[txk],b=y[txk];if(txk=='ago'){a=a<0?1e18:a;b=b<0?1e18:b;}return (a>b?1:a<b?-1:0)*txd;});
 $('txb').innerHTML=l.slice(0,5000).map(t=>`<tr><td>${t.preview?`<a href="${esc(t.preview)}" target="_blank"><img class="th" src="${esc(t.preview)}"></a>`:''}</td><td class="mono">${t.id}</td><td class="mono">${t.addr}</td><td>${t.kind}</td><td class="mono">${t.w}x${t.h}${t.d>1?'x'+t.d:''}</td><td>${t.mips}</td><td>${t.format}</td><td>${t.host?'<span class="tag ok">host</span>':'<span class="tag no">no host</span>'}${t.fills?(t.filled?'<span class="tag ok">filled</span>':'<span class="tag no">not filled</span>'):'<span class="tag mid">card writes</span>'}${t.refused?'<span class="tag no">refused</span>':''}${t.gone?'<span class="tag mid">let go once</span>':''}${t.placed?'<span class="tag mid">placed</span>':''}</td><td>${t.mb}</td><td>${t.ago<0?'never':t.ago}</td></tr>`).join('');}
for(const th of document.querySelectorAll('#tex th'))th.onclick=()=>{const k=th.dataset.k;txd=(txk==k)?-txd:1;txk=k;drawTex();};
$('txf').oninput=drawTex;$('txs').onchange=drawTex;drawTex();
// traces
const asf=v=>{const b=new DataView(new ArrayBuffer(4));b.setUint32(0,v);const f=b.getFloat32(0);return Number.isFinite(f)&&Math.abs(f)<1e9&&(Math.abs(f)>1e-6||f==0)?+f.toPrecision(6):''};
$('trb').innerHTML=TRACES.length?TRACES.map(t=>`<h3>${esc(t.stage)} ${esc(t.hash)} · tag ${t.tag} · lane ${t.lane}</h3><p class="note">${t.steps.length} register writes. Click a row for the instruction's place in the program.</p><table><thead><tr><th>#</th><th>offset</th><th>instruction</th><th>reg</th><th>value</th><th>as float</th></tr></thead><tbody>${t.steps.slice(0,20000).map(s=>`<tr><td>${s[0]}</td><td class="mono">${s[1].toString(16).padStart(6,'0')}</td><td class="mono">${esc(s[2])}</td><td class="mono">${s[3]}</td><td class="mono">0x${(s[4]>>>0).toString(16)}</td><td class="mono">${asf(s[4])}</td></tr>`).join('')}</tbody></table><details><summary>whole program (${t.code.length} instructions)</summary><pre class="mono">${t.code.map(c=>c[0].toString(16).padStart(6,'0')+': '+esc(c[1])).join('\n')}</pre></details>`).join(''):'<p>No traces. Start with VICTORY_SHADER_TRACE=&lt;shader hash&gt; and record a frame with the mouse over a pixel that shader draws.</p>';
// targets
$('tgb').innerHTML=PICTURES.map(p=>`<figure><a href="${esc(PREFIX+p)}" target="_blank"><img src="${esc(PREFIX+p)}" loading="lazy"></a><figcaption>${esc(p)}</figcaption></figure>`).join('')||'<p>No target pictures.</p>';
</script></body></html>
"##;
