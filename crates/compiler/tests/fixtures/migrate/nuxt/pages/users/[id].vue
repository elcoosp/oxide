<script setup>
const user = await useAsyncData('user', () => queryCollection('users').item(useRoute().params.id))
</script>
<template><h1>{{ user.name }}</h1></template>
