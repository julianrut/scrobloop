import asyncio
import ssl
import certifi
import aiohttp
import sys
from shazamio import Shazam
import json

# Needs Python 3.12 since it requires audioop

async def main():
    ssl_context = ssl.create_default_context(cafile=certifi.where())
    connector = aiohttp.TCPConnector(ssl=ssl_context)
    shazam = Shazam()
    shazam.http_client._connector = connector
    out = await shazam.recognize(sys.argv[1])
    print(json.dumps(out, indent=2))

asyncio.run(main())