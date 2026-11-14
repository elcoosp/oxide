module.exports = {
  images: { remotePatterns: [{ protocol: 'https', hostname: 'cdn.example.com' }] },
  async redirects() { return [] },
}
