# G0-1 — 2026-08-20 toplantı kavramlarının spec denetimi

**Girdi:** `gorevlendirme.md` § "G0-1 · `CLAUDE.md` + teknik spesifikasyona yeni kavramları ekle"
**Yöntem:** görevlendirmedeki 7 kavramın her biri kodda ve kanonik spec'te (`docs/spec/`) tek
tek arandı. "Dokümanda yazılı değil" iddiası kavram bazında doğrulandı — **iddianın büyük
kısmı yanlış çıktı: yedi kavramın dördü zaten hem kodda hem spec'te vardı.**

**Durum kodları:** ✅ zaten vardı · 📝 vardı ama kanonik spec'e yazılmamıştı → BU OTURUMDA yazıldı
· ❓ gerçekten yok → karar bekliyor (spec'e TEK TARAFLI yazılmadı, bkz. `docs/spec/README.md`
"Emin olamadığın tasarım kararında iki seçeneği açıkla ve sor; spec'i kendi başına genişletme")

---

## 1. Denetim tablosu

| # | G0-1 kavramı | Durum | Kanıt |
|---|---|---|---|
| 1 | `wf_admin` bloğu (WFD üst seviye) | ✅ | `schema.json:142` · `terminology.md` §WF_ADMIN · `decisions.md:1693` (T‑A5) · `CLAUDE.md` §"WF Admin" · `docs/superpowers/specs/2026-08-11-wf-admin-design.md` |
| 2a | Hedef seçimli aksiyon ("global aksiyon" = GLB) | 📝 | `schema.json:1013` (`wftGlobalTargets`) · `wfd_v22.rs:765` (`Wft::Targets`) · `pipeline.rs:157` · `CLAUDE.md` §"API sözleşmesi v2". **Kanonik `terminology.md`'de YOKTU** → yazıldı |
| 2b | Yetkiliye ait sistem aksiyonu kümesi (`cancel`, `send_to_start`, `assign_from_pool` …) | ❓ | Motorda YOK. `wf_admin` üç yetki verir (claim devri · escalation fire/skip · görme) ve `terminology.md` açıkça "**aksiyon yetkisi VERMEZ**" diyor |
| 3 | `send_back` + çok hedefli `wft` | 📝 / kısmen ❓ | Çok hedefli `wft` VAR (2a). `send_back` ayrı bir aksiyon TİPİ değil, `wft: {targets}` taşıyan normal bir aksiyon — bu J-1 kararıyla birebir uyuşuyor → yazıldı. **Hedef listesinin WFAH'tan türetilmesi YOK** (liste statik, ileriye gönderme engellenmiyor) |
| 4a | Node kimliği + `label` | ✅ | Kimlik = `nodes` object key, tasarımcı verir (`schema.json` `propertyNames: idName`); `nodes.<key>.label` opsiyonel display. `terminology.md` §NODE + `runtime-semantics.md` §2b |
| 4b | Sıra numarası (`seq`) | ⛔ | Yok ve **EKLENMEYECEK** — kullanıcı kararı 2026-08-21; gerekçe §3, kayıt `decisions.md` "G0-1 spec denetimi" Karar 5 |
| 5 | Aksiyon `label` alanı | ✅ | `schema.json:684` (`actionDef.label`) + wire'da `Ref { id, label }` her yerde (`wfe/src/executor.rs:135`); etiket ASLA null dönmez (`display::humanize_key`). "Editörde çalışmıyor" ise **editör hatası, spec boşluğu değil** |
| 6 | Havuz / claim / release semantiği | 📝 / kısmen ❓ | Havuz + claim + `can_claim` ayrımı + `reassign` + `claim_timeout` VAR (`portal/pool.rs:104`, `routes/wfe.rs:51-52`). **Kanonik spec'te toplu bir bölüm YOKTU** → yazıldı. **Aktörün kendi claim'ini bıraktığı `release` ucu YOK** |
| 6b | "Lock süresiz" (5 dk kaldırıldı) | ✅ | WFD **taslak** kilidi 2026-08-18'de süresizleşti (`CLAUDE.md` §"WFD taslak kilidi"). WFE claim'inde de sabit TTL yok; süre yalnız tasarımcının `nodes.<key>.claim_timeout`'undan gelir |
| 7 | `x-visibility`'nin node/state bazlı hâli | ❓ | Gerçekten yok. `schema.json:474` `$defs/visibility` = `{c_orgu, c_r, c_u, c_a}`, **`when` YOK ve dizi değil**. Karşılaştır: `$defs/caGrantRule` (`schema.json:1482`) `when` taşır — `listable`/`wf_admin` orada |

## 2. Bu oturumda spec'e yazılanlar

Hepsi **var olan davranışın** kanonik spec'e taşınmasıdır; yeni davranış tanımlanmadı.
Dosyalar iki kopyada da (`agnoflow-backend/docs/spec/`, `agnoflow-frontend/docs/spec/`)
aynı şekilde güncellendi.

1. `terminology.md` §"WFT / TRIGGER / AUTOEXEC / PIPELINE" — WFT form listesi **üç formdan
   altı forma** çıkarıldı (`{targets}`, `{parallel}`, `{collapse}` eksikti; belge hâlâ
   "v2.1 ile aynıdır" diyordu).
2. `terminology.md` §"GLB — HEDEF SEÇİMLİ AKSİYON (WFD içi 'geri gönderme')" — JSON örneği,
   `target` wire kuralı ve üç hata kodu, `target`ın action input OLMADIĞI, hedef etiketinin
   node `label`'ı olduğu, listenin STATİK olduğu, validator kodları ve **adlandırma tuzağı**
   (koddaki "global aksiyon" ile toplantıdaki "global aksiyon" AYNI ŞEY DEĞİL).
3. `terminology.md` §"API GÖSTERİM SÖZLEŞMESİ — `Ref { id, label }`" — kimlik/gösterim
   ayrımı, etiketin nereden geldiği, `Ref` dönen tüm yüzeylerin listesi, çok dilli label
   OLMADIĞI.
4. `terminology.md` §"HAVUZ / CLAIM / ASSIGNMENT" — üç kapının (görünmek / claim etmek /
   ACT almak) tablosu, CAS ve idempotent re-claim, `claim_timeout`, `reassign` (`to: null`
   = havuza bırakma), **`release` ucunun olmadığı**.
5. `docs/spec/README.md` "Model Özeti" — aynı eksik `wft` satırı düzeltildi.
6. `docs/spec/decisions.md` — "G0-1 spec denetimi… (2026-08-20)" kaydı: denetim sonucu,
   kök neden (kanonik yer `terminology.md`'dir), "global aksiyon" ad çakışması, B-4/L-2'nin
   cevabı (ayrı `no_send_back` bayrağı YOK) ve `seq` önerisi. **Yalnız backend (kanonik)
   kopyada** — frontend kopyası ~600 satır geride, tek yönlü yenilemesi ayrı görev (§5).

## 3. `seq` (sıra numarası) — REDDEDİLDİ (kullanıcı, 2026-08-21)

Görevlendirme B-1'de node'a `seq` (1'den başlayan topolojik sıra) isteniyor; gerekçe
"UI 'bir öncekine gönder' = `seq - 1`, 'başa gönder' = `seq = 1` diyebilsin".

**Karar: eklenmiyor** (kullanıcı, 2026-08-21 — *"Seq kararını reddetmiştik"*). Üç gerekçe:

1. **Topolojik sıra bu grafta TEK DEĞİL.** Akış bir zincir değil yönlü graf: fork/join
   (`wftParallel`), koşullu dallar, escalation kenarları ve WFC dönüşleri var. Paralel iki
   kolun node'ları arasında "hangisi önce" sorusunun doğru cevabı yoktur — `seq` üretmek
   için keyfî bir tie-break seçmek ve onu kalıcı sözleşme yapmak gerekir. `seq - 1`
   paralel kolda YANLIŞ node'u gösterir.
2. **İkinci bir kimlik açar.** `node key` tasarımcının verdiği stabil kimlik; bu 2026-08-12'de
   bilinçli olarak slug'dan koparıldı (`runtime-semantics.md` §2a/§2b). `seq` bir node
   eklendiğinde/silindiğinde KAYAR — yani stabil olmayan ikinci bir kimlik olur. WFAH'ta
   `seq` zaten var (WFAH kaydının sıra numarası) ve o farklı bir eksen; aynı adı iki farklı
   şey için kullanmak ifade dilinde (`$wfah` izdüşümü `{seq, action, actor, input, at}`)
   doğrudan karışıklık üretir.
3. **UI'nin ihtiyacı `seq` DEĞİL, "geçmiş node listesi" — ve o zaten dönüyor.**
   `GET /wfe/{id}` yanıtında `path[]` var (`WfeView.path`, `wfe/src/executor.rs:387`):

   ```json
   "path": [
     { "seq": 1, "action": { "id": "start", "label": "Akışı Hazırla" },
       "from": null, "to": { "id": "self__creditAnalyst", "label": "Analist Havuzu" },
       "at": "..." },
     { "seq": 2, "action": { "id": "analyst_submit", "label": "İncelemeyi Bitir" },
       "from": { "id": "self__creditAnalyst", "label": "Analist Havuzu" },
       "to": { "id": "parent__creditDeptManager", "label": "Kredi Müdürü" },
       "at": "..." }
   ]
   ```

   "Başa gönder" = `path[0].to`, "bir öncekine gönder" = son adımın `from`'u. Statik belge
   sırası yerine **o instance'ın gerçek geçmişi** kullanılır — hem doğru olan bu, hem de
   görevlendirmenin B-2 kabul kriteriyle ("hedefler yalnızca WFAH'ta geçmiş node'lar
   arasından çıkıyor") tutarlı olan bu. Dikkat: `PathStep.seq` **WFAH sıra numarasıdır**,
   node'un değil — istenen `seq` bu alanla aynı adı taşıyıp başka şeyi anlatacaktı.

İhtiyaç gerçek, karşılığı farklı: "başa / bir öncekine gönder" `path[]` ile, hedef
kümesinin daraltılması **K-2** ile çözülür. `nodes.<key>`e sıra alanı eklenmez; şemada
`seq` diye bir node alanı YOKTUR.

## 4. Karar bekleyenler (spec'e TEK TARAFLI yazılmadı)

| # | Konu | Seçenekler |
|---|---|---|
| K-1 | Yetkili sistem aksiyonları (`cancel`, `send_to_start`, `assign_from_pool`, `reclaim_to_pool`) + `wf_admin.allowed_global_actions` — **ad çakışması 2026-08-21'de ÇÖZÜLDÜ** ("global aksiyon" artık YALNIZ bu küme için ayrıldı; eski mekanizma `send_back` oldu), küme kendisi hâlâ açık | (a) `wf_admin`'in kapsamını genişlet — mevcut "**aksiyon yetkisi VERMEZ**" değişmezi (T‑A5) KIRILIR, `cancel`/`send_to_start` yeni WFE durumu + DynCtx revizyonu getirir. (b) Bunları WFD'de normal aksiyon olarak tanımlat (K bölümünün "sistem default review node'u" reddiyle aynı gerekçe). (c) Yalnız `cancel`i ekle, gezinme aksiyonlarını (b)'ye bırak. **Ayrıca adlandırma:** "global aksiyon" adı GLB'de KULLANILIYOR; bu küme için farklı bir ad gerekir (öneri: `admin_actions`) |
| ~~K-2~~ | ~~Hedef listesi WFAH ile kesişsin mi~~ | **KAPANDI (2026-08-21): (a) yapıldı.** Menü çalışma anında `targets ∩ visited_nodes`; hiç hedef kalmazsa aksiyon HİÇ sunulmaz, `apply` aynı süzgeci kapı olarak sorar (`action.target_invalid`). Start node'u kümeye `wfd.start[]`ten eklenir ("başa gönder" buna bağlı). Kayıt: `docs/spec/decisions.md` → "Rezerve `send_back` GERİ ALINDI + K-2" |
| ~~K-3~~ | ~~Hedef başına serbest metin label~~ | **KAPANDI (2026-08-21):** eklendi — `$defs/sendBackTarget.label`, opsiyonel, boşsa node label'ına düşer. Aksiyonun adı da sabitlendi (rezerve `send_back`, gösterim "Geri Gönder"). Kayıt: `docs/spec/decisions.md` → "Global aksiyon adı BIRAKILDI" |
| K-4 | `x-visibility` node/state bazlı | (a) `$defs/visibility`'yi diziye çevir + `when` ekle (`caGrantRule` deseni) — `V(dynctx, actor)` imzası `wfes` almak üzere genişler, alan bazlı gizlilik SQL projeksiyonuna dokunur. (b) Alan bazlı kalsın, node bazlı ihtiyaç ayrı node + ayrı ctx alanıyla çözülsün. **Not:** `when`'li kuralların önce, `when`'sizin varsayılan olması sırası şemada değil VALIDATOR'da zorlanmalı |
| K-5 | `release` (aktör kendi claim'ini bırakır) ucu | (a) `POST /wfe/{id}/release` ekle — kapı "çağıran claim sahibi mi". (b) Mevcut `reassign to:null` yolu yeterli sayılsın (ama kapısı `node.reassign ∪ wf_admin`, sahibin kendisi DEĞİL) |
| K-6 | Label i18n (`{"tr": ..., "en": ...}`) | Bugün `label` düz string. (a) union tipe çevir — `display` modülü dil bağlamı almak zorunda kalır (bugün almıyor). (b) tek dil kalsın, çeviri istemcide sözlükle yapılsın |


## 5. Spec'in iki kopyası ayrışmıştı — SENKRONLANDI (2026-08-21)

`docs/spec/README.md` iki kopyanın "**birebir aynı**" tutulduğunu söylüyor; değildi:

| Dosya | Frontend kopyasının durumu (senkron öncesi) |
|---|---|
| `terminology.md` | 143 satır fark — node kimliği hâlâ **`slug(c_a)`'dan türetiliyor** (2026-08-12'de KALDIRILDI), çapasız C_A yok |
| `runtime-semantics.md` | 147 satır fark — §2a "validator slug'ı yeniden hesaplayıp karşılaştırır" diyor (yanlış), node/terminal `listable` matcher'ları yok |
| `decisions.md` | **682 satır fark** — T‑A5 (WF Admin), T‑B4 (taslak kilidi), görünürlük projeksiyonu, node/terminal `listable`, `duplicate_c_a`, `format` göçü, runtime tip denetimi kararlarının HİÇBİRİ yok |
| `migration-notes.md` | 77 satır fark |
| `reference-types.rs` | 202 satır fark — eski `ListableRule`, bare node key formu |
| `README.md` | 2 satır fark |
| `schema.json`, `examples/` | senkrondu (fark yok) |

Frontend kopyasında **özgün içerik yoktu** — tüm farklar eski sürümdü (`diff | grep '^>'`
ile tek tek bakıldı), o yüzden birleştirme gerekmedi.

**Yapılan:** altı dosya backend → frontend **tek yönlü** kopyalandı; `diff -rq` ile dizin
artık birebir aynı. **Yön kalıcıdır: kanon backend `docs/spec/`, frontend kopyası
tüketicidir.** Frontend'de fark edilen bir düzeltme önce backend'e yazılır, sonra
kopyalanır.

Not: frontend `CLAUDE.md` (1135 satır) GÜNCEL — GLB, `Ref {id,label}`, aksiyon kimliği
(2026-08-20), terminal `label` kararlarını doğru anlatıyor. Bayat olan yalnız `docs/spec/`
kopyasıydı; yani editör deposu doğru davranıp yanlış kaynağı taşıyordu.
