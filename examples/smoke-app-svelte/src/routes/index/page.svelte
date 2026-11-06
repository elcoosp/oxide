<script>
  import HmrTarget from '../../components/hmr-target.svelte'

  export let data = { message: '' }
</script>

<main>
  <h1>Home</h1>
  <p>{data.message}</p>
  <HmrTarget />
</main>
