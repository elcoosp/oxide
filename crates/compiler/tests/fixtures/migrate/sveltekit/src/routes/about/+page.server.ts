import { api } from '$lib/api'
export const load = async () => ({ title: api.title() })
