// Builds the website into dist/: the homepage, one page per docs/*.md in the
// order docs/nav.json gives, the assets, and install.sh from the repository
// root. Every problem found is printed and the build exits 1, so a broken
// link never reaches Pages.
//
//   node build.mjs

import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Marked, Renderer } from 'marked';

const site = dirname(fileURLToPath(import.meta.url));
const dist = join(site, 'dist');
const home = 'https://saifulapm.github.io/amx/';
const repo = 'https://github.com/saifulapm/amx';
// The logo mark, as index.html draws it: a tmux window of three panes, one lit.
const mark =
  '<svg class="mark" viewBox="0 0 24 24" aria-hidden="true"><rect x="1" y="1" width="22" height="22" rx="3"/><path d="M12 1v22M12 12h11"/><rect class="lit" x="13" y="2" width="9" height="9"/></svg>';

const errors = [];
const nav = JSON.parse(readFileSync(join(site, 'docs/nav.json'), 'utf8'));
const listed = new Set(nav.map((entry) => entry.slug));

for (const file of readdirSync(join(site, 'docs'))) {
  if (file.endsWith('.md') && !listed.has(file.slice(0, -3))) {
    errors.push(`docs/${file} is not listed in docs/nav.json`);
  }
}

const pages = [];
for (const { slug, title } of nav) {
  const file = join(site, 'docs', `${slug}.md`);
  if (!existsSync(file)) {
    errors.push(`docs/nav.json lists "${slug}", but docs/${slug}.md does not exist`);
    continue;
  }
  const source = readFileSync(file, 'utf8');
  if (!source.startsWith('# ')) errors.push(`docs/${slug}.md must start with a "# Title" line`);
  pages.push({ slug, navTitle: title, ...render(source, slug) });
}

const bySlug = new Map(pages.map((page) => [page.slug, page]));
for (const page of pages) {
  for (const { href, slug, fragment } of page.links) {
    const target = bySlug.get(slug);
    if (slug === undefined) {
      errors.push(`docs/${page.slug}.md: "${href}" is a relative link to no docs page; link to another page as "name.md", anything else by its full URL`);
    } else if (!target) {
      errors.push(`docs/${page.slug}.md: "${href}" links to docs/${slug}.md, which does not exist`);
    } else if (fragment && !target.ids.has(fragment)) {
      errors.push(`docs/${page.slug}.md: "${href}" links to #${fragment}, which is no heading in docs/${slug}.md`);
    }
  }
}

if (errors.length) {
  for (const error of errors) console.error(`error: ${error}`);
  process.exit(1);
}

rmSync(dist, { recursive: true, force: true });
mkdirSync(join(dist, 'docs'), { recursive: true });
cpSync(join(site, 'assets'), join(dist, 'assets'), { recursive: true });
cpSync(join(site, 'style.css'), join(dist, 'style.css'));
cpSync(join(site, '..', 'install.sh'), join(dist, 'install.sh'));
writeFileSync(join(dist, 'index.html'), homepage());
pages.forEach((page, i) => {
  mkdirSync(join(dist, 'docs', page.slug));
  writeFileSync(join(dist, 'docs', page.slug, 'index.html'), docPage(page, pages[i - 1], pages[i + 1]));
});
writeFileSync(join(dist, 'docs', 'index.html'), redirect(`${pages[0].slug}/`));
console.log(`built dist/ with ${pages.length} docs pages`);

// The homepage as written.
function homepage() {
  return readFileSync(join(site, 'index.html'), 'utf8');
}

// One Markdown page as HTML, with the ids its headings got and every relative
// link it makes, for checking once all pages are known.
function render(source, slug) {
  const ids = new Set();
  const links = [];
  let title = null;
  let description = null;
  const marked = new Marked({ gfm: true });
  marked.use({
    renderer: {
      heading({ tokens, depth }) {
        const inner = this.parser.parseInline(tokens);
        const text = plain(this.parser.parseInline(tokens, this.parser.textRenderer));
        if (depth === 1) {
          title ??= text;
          return `<h1>${inner}</h1>\n`;
        }
        let id = slugify(text);
        for (let n = 1; ids.has(id); n++) id = `${slugify(text)}-${n}`;
        ids.add(id);
        return `<h${depth} id="${id}"><a class="anchor" href="#${id}" aria-label="Link to this section">#</a>${inner}</h${depth}>\n`;
      },
      paragraph(token) {
        description ??= plain(this.parser.parseInline(token.tokens, this.parser.textRenderer));
        return false;
      },
      link(token) {
        return Renderer.prototype.link.call(this, { ...token, href: rewrite(token.href, slug, links) });
      },
      table(token) {
        return `<div class="table">${Renderer.prototype.table.call(this, token)}</div>\n`;
      },
    },
  });
  const html = marked.parse(source);
  return { html, ids, links, title: title ?? slug, description: description ?? '' };
}

// A link as the built page needs it. `name.md` and `name.md#part` become
// `../name/` and `../name/#part`; `../name/` is kept. Every relative link is
// recorded so it can be checked against the pages that exist.
function rewrite(href, slug, links) {
  if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(href)) return href;
  if (href.startsWith('#')) {
    links.push({ href, slug, fragment: href.slice(1) });
    return href;
  }
  const page = /^(?:\.\/)?([\w-]+)\.md(?:#(.*))?$/.exec(href) ?? /^\.\.\/([\w-]+)\/?(?:#(.*))?$/.exec(href);
  if (!page) {
    links.push({ href });
    return href;
  }
  const [, target, fragment] = page;
  links.push({ href, slug: target, fragment });
  return `../${target}/${fragment ? `#${fragment}` : ''}`;
}

// GitHub's heading slug: lower case, punctuation dropped, each space a hyphen.
function slugify(text) {
  return text.toLowerCase().replace(/[^\p{L}\p{M}\p{N}\p{Pc} -]/gu, '').replace(/ /g, '-');
}

// Text the text renderer produced, with its HTML escapes undone.
function plain(html) {
  const entities = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };
  return html.replace(/&(?:amp|lt|gt|quot|#39);/g, (entity) => entities[entity]).trim();
}

function escape(text) {
  return text.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
}

function summary(paragraph) {
  const text = paragraph.replace(/\s+/g, ' ');
  return text.length <= 160 ? text : `${text.slice(0, 157).replace(/\s+\S*$/, '')}...`;
}

function docPage(page, prev, next) {
  const pagesList = nav
    .map(({ slug, title }) => {
      const current = slug === page.slug ? ' aria-current="page"' : '';
      return `<li><a href="../${slug}/"${current}>${escape(title)}</a></li>`;
    })
    .join('\n          ');
  const list = `<ul>\n          ${pagesList}\n        </ul>`;
  const step = (other, rel, label) =>
    other
      ? `<a class="${rel}" href="../${other.slug}/" rel="${rel}"><span>${label}</span>${escape(other.navTitle)}</a>`
      : '<span></span>';

  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${escape(page.title)} | amx</title>
  <meta name="description" content="${escape(summary(page.description))}">
  <meta name="theme-color" content="#1e1f22" media="(prefers-color-scheme: dark)">
  <meta name="theme-color" content="#fafaf9" media="(prefers-color-scheme: light)">
  <link rel="canonical" href="${home}docs/${page.slug}/">
  <link rel="icon" href="../../assets/favicon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="../../assets/apple-touch-icon.png">
  <link rel="preload" href="../../assets/fonts/literata-400.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="preload" href="../../assets/fonts/plex-mono-400.woff2" as="font" type="font/woff2" crossorigin>
  <link rel="stylesheet" href="../../style.css">
  <meta property="og:type" content="article">
  <meta property="og:site_name" content="amx">
  <meta property="og:title" content="${escape(page.title)} | amx">
  <meta property="og:description" content="${escape(summary(page.description))}">
  <meta property="og:url" content="${home}docs/${page.slug}/">
  <meta property="og:image" content="${home}assets/og.png">
  <meta name="twitter:card" content="summary_large_image">
</head>
<body>
  <header class="topbar wrap">
    <a class="brand" href="../../">${mark}amx</a>
    <nav class="topnav" aria-label="Site">
      <a href="../${pages[0].slug}/" aria-current="true">Docs</a>
      <a href="${repo}">GitHub</a>
    </nav>
  </header>
  <div class="docs wrap">
    <nav class="sidebar" aria-label="Docs">
      ${list}
    </nav>
    <main class="doc">
      <details class="pages">
        <summary>Contents</summary>
        <nav aria-label="Docs">
        ${list}
        </nav>
      </details>
      <article class="prose">
${page.html}      </article>
      <nav class="pager" aria-label="Previous and next page">
        ${step(prev, 'prev', 'Previous')}
        ${step(next, 'next', 'Next')}
      </nav>
    </main>
  </div>
  <footer class="footer wrap">
    <a class="brand" href="../../">${mark}amx</a>
    <p>MIT or Apache-2.0, at your option. <a href="${repo}">Source on GitHub</a>.</p>
  </footer>
</body>
</html>
`;
}

function redirect(to) {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>amx docs</title>
  <meta http-equiv="refresh" content="0; url=${to}">
  <link rel="canonical" href="${home}docs/${to}">
</head>
<body>
  <p><a href="${to}">Go to the amx docs</a>.</p>
</body>
</html>
`;
}
