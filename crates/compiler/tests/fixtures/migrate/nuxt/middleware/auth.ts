export default defineNuxtRouteMiddleware((to) => {
  if (!to.meta.auth) return navigateTo('/login')
})
