import DefaultTheme from 'vitepress/theme'
import './custom.css'

export default {
    ...DefaultTheme,
    enhanceApp({ app, router, siteData }: { app: any, router: any, siteData: any }) {
        // call the base themes enhanceApp
        DefaultTheme.enhanceApp({ app, router, siteData })
    }
}
