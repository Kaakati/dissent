// A message's markdown as React elements. The tree comes from lib/markdown.ts; nothing here builds HTML from the text. Text
// runs go through the caller's renderer, so mention pills and search matches still show inside formatted text. A masked
// link shows its address on hover, and asks before it opens when its label names another site.
import { Fragment, useEffect, useMemo, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react';
import { confirm } from '@/lib/dialog';
import { type CodeNode, highlightCode } from '@/lib/code-highlight';
import { inlinePlain, mismatchedHost, parseMarkdown, safeHref, type Block, type Inline } from '@/lib/markdown';
import { cn } from '@/lib/utils';

// Renders one run of plain text: mentions and search matches.
export type TextRender = (text: string) => ReactNode[];

// A run with its line breaks as <br />, each line through the caller's renderer.
function Run({ text, render }: { text: string; render: TextRender }) {
  return (
    <>
      {text.split('\n').map((line, i) => (
        <Fragment key={i}>
          {i > 0 ? <br /> : null}
          {render(line)}
        </Fragment>
      ))}
    </>
  );
}

// A link that says one site and goes to another asks first. Otherwise it opens as links in messages do.
function openLink(href: string): void {
  const a = document.createElement('a');
  a.href = href;
  a.target = '_blank';
  a.rel = 'noopener noreferrer';
  a.click();
}

function MaskedLink({ href, label, render, path }: { href: string; label: Inline[]; render: TextRender; path: string }) {
  const text = inlinePlain(label);
  const mismatch = mismatchedHost(text, href);
  const onClick = async (e: MouseEvent<HTMLAnchorElement>) => {
    if (!mismatch) return;
    e.preventDefault();
    const target = new URL(href).host;
    if (await confirm({ message: `This link says ${text} but goes to ${target}. Open it?`, okLabel: 'Open' })) openLink(href);
  };
  return (
    <a className="md-link" href={href} target="_blank" rel="noopener noreferrer" title={href} onClick={(e) => void onClick(e)}>
      <InlineNodes nodes={label} render={render} path={`${path}.l`} />
    </a>
  );
}

// Hidden until pressed. Revealing is one way, as Discord does it; the text stays selectable and in the DOM.
function Spoiler({ children }: { children: ReactNode }) {
  const [shown, setShown] = useState(false);
  const reveal = (e: { stopPropagation: () => void }) => {
    e.stopPropagation();
    setShown(true);
  };
  const onKey = (e: KeyboardEvent<HTMLSpanElement>) => {
    if (!shown && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      reveal(e);
    }
  };
  return (
    <span
      className={cn('md-spoiler', shown && 'revealed')}
      role={shown ? undefined : 'button'}
      tabIndex={shown ? undefined : 0}
      aria-label={shown ? undefined : 'Spoiler. Press to reveal'}
      onClick={shown ? undefined : reveal}
      onKeyDown={shown ? undefined : onKey}
    >
      {children}
    </span>
  );
}

function InlineNode({ node, render, path }: { node: Inline; render: TextRender; path: string }): ReactNode {
  switch (node.kind) {
    case 'text':
      return <Run text={node.text} render={render} />;
    case 'url':
      return (
        <a className="md-link" href={node.href} target="_blank" rel="noopener noreferrer" title={node.href}>
          <Run text={node.href} render={render} />
        </a>
      );
    case 'code':
      return <code className="md-code">{node.text}</code>;
    case 'link': {
      const href = safeHref(node.href);
      return href ? (
        <MaskedLink href={href} label={node.label} render={render} path={path} />
      ) : (
        <InlineNodes nodes={node.label} render={render} path={path} />
      );
    }
    case 'em':
      return <em><InlineNodes nodes={node.children} render={render} path={path} /></em>;
    case 'strong':
      return <strong><InlineNodes nodes={node.children} render={render} path={path} /></strong>;
    case 'underline':
      return <u><InlineNodes nodes={node.children} render={render} path={path} /></u>;
    case 'strike':
      return <s><InlineNodes nodes={node.children} render={render} path={path} /></s>;
    case 'spoiler':
      return <Spoiler><InlineNodes nodes={node.children} render={render} path={path} /></Spoiler>;
  }
}

function InlineNodes({ nodes, render, path }: { nodes: Inline[]; render: TextRender; path: string }) {
  return (
    <>
      {nodes.map((n, i) => (
        <InlineNode key={i} node={n} render={render} path={`${path}.${i}`} />
      ))}
    </>
  );
}

function BlockNode({ block, render, path }: { block: Block; render: TextRender; path: string }) {
  switch (block.kind) {
    case 'para':
      return <p className="md-p"><InlineNodes nodes={block.inline} render={render} path={path} /></p>;
    case 'heading':
      return (
        <div role="heading" aria-level={block.level} className={`md-h${block.level}`}>
          <InlineNodes nodes={block.inline} render={render} path={path} />
        </div>
      );
    case 'subtext':
      return <div className="md-subtext"><InlineNodes nodes={block.inline} render={render} path={path} /></div>;
    case 'quote':
      return (
        <blockquote className="md-quote">
          {block.blocks.map((b, i) => (
            <BlockNode key={i} block={b} render={render} path={`${path}.${i}`} />
          ))}
        </blockquote>
      );
    case 'list': {
      const List = block.ordered ? 'ol' : 'ul';
      return (
        <List className="md-list">
          {block.items.map((item, i) => (
            <li key={i}>
              <InlineNodes nodes={item.inline} render={render} path={`${path}.${i}`} />
              {item.children.map((b, j) => (
                <BlockNode key={j} block={b} render={render} path={`${path}.${i}.${j}`} />
              ))}
            </li>
          ))}
        </List>
      );
    }
    case 'code':
      return <CodeBlock lang={block.lang} text={block.text} />;
  }
}

// The spans of a highlighted block. Only the class names highlight.js gave are used, and the text is React text.
function codeNodes(nodes: CodeNode[]): ReactNode[] {
  return nodes.map((n, i) => (typeof n === 'string' ? n : <span key={i} className={n.cls}>{codeNodes(n.children)}</span>));
}

// A fenced block. Plain until its language is loaded (highlight.js loads on the first block that names one), then coloured.
function CodeBlock({ lang, text }: { lang: string; text: string }) {
  // The colouring is kept with the text it was made from, so a block that changes never shows the old colours.
  const source = `${lang}
${text}`;
  const [done, setDone] = useState<{ source: string; tree: CodeNode[] } | null>(null);
  useEffect(() => {
    let live = true;
    if (lang) {
      void highlightCode(text, lang).then((t) => {
        if (live && t) setDone({ source, tree: t });
      });
    }
    return () => {
      live = false;
    };
  }, [lang, text, source]);
  const tree = done && done.source === source ? done.tree : null;
  return (
    <div className="md-codeblock">
      {lang ? <div className="md-codelang">{lang}</div> : null}
      <pre>
        <code>{tree ? codeNodes(tree) : text}</code>
      </pre>
    </div>
  );
}

// The body of a message. Parsed once per text; the parse is cached in lib/markdown.ts as well.
export function MarkdownBody({ text, render }: { text: string; render: TextRender }) {
  const blocks = useMemo(() => parseMarkdown(text), [text]);
  return (
    <>
      {blocks.map((b, i) => (
        <BlockNode key={i} block={b} render={render} path={`${i}`} />
      ))}
    </>
  );
}
