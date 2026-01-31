import DefaultTheme from 'vitepress/theme'
import './custom.css'
import { initComponent } from 'vitepress-mermaid-preview/component';
import 'vitepress-mermaid-preview/dist/index.css';

export default {
    ...DefaultTheme,
    enhanceApp({ app, router, siteData }: { app: any, router: any, siteData: any }) {
        // call the base themes enhanceApp
        DefaultTheme.enhanceApp({ app, router, siteData })
        // Register Mermaid component globally
        initComponent(app);
    }
}
