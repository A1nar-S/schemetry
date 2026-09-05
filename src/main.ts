import './app.css';
import { mount } from 'svelte';
import App from './App.svelte';

const app = mount(App, {
  target: document.getElementById('root') as HTMLElement,
});

export default app;
