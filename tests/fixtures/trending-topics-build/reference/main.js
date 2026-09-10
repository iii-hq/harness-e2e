import feed from '../content/feed.json';
import './style.css';

const app = document.querySelector('#app');
const topics = [...feed.topics].sort((a, b) => a.rank - b.rank);

function element(tag, text, attributes = {}) {
  const node = document.createElement(tag);
  if (text !== undefined) node.textContent = text;
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  return node;
}

function render() {
  const topic = topics.find(({ id }) => `/posts/${id}` === location.pathname);
  const main = element('main');
  app.replaceChildren(main);

  if (topic) {
    document.title = `${topic.title} — Trending topics`;
    main.className = 'article';
    main.append(
      element('a', 'All topics', { href: '/' }),
      element('p', `Rank ${topic.rank}`, { class: 'eyebrow' }),
      element('h1', topic.title),
      element('p', topic.source_body, { class: 'body' }),
      element('a', 'Source', { href: topic.url }),
    );
    return;
  }

  document.title = 'Trending topics';
  main.append(
    element('p', `Edition ${feed.edition}`, { class: 'eyebrow' }),
    element('h1', 'Trending topics'),
    element('p', 'Six stories shaping the conversation.', { class: 'intro' }),
  );

  const list = element('ol');
  for (const item of topics) {
    const link = element('a', item.title, { href: `/posts/${item.id}` });
    const article = element('article');
    article.append(element('p', `Rank ${item.rank}`, { class: 'rank' }), link);
    const listItem = element('li', undefined, { value: item.rank });
    listItem.append(article);
    list.append(listItem);
  }
  main.append(list);
}

render();
