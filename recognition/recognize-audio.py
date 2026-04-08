import asyncio
import ssl
import certifi
import aiohttp
import sys
import json
from shazamio import Shazam

# Needs Python 3.12 since it requires audioop
# File path in argument

async def main():
    ssl_context = ssl.create_default_context(cafile=certifi.where())
    connector = aiohttp.TCPConnector(ssl=ssl_context)
    shazam = Shazam()
    shazam.http_client._connector = connector
    out = await shazam.recognize(sys.argv[1])
    print(format_shazamio_output(out))

def format_shazamio_output(output):
    track = output.get("track", {})
    if not track:
        return "No match found."

    sections = track.get("sections", [])
    metadata = {}
    for section in sections:
        if section.get("type") == "SONG":
            for item in section.get("metadata", []):
                metadata[item["title"]] = item["text"]

    hub = track.get("hub", {})
    apple_music_url = None
    for option in hub.get("options", []):
        for action in option.get("actions", []):
            if action.get("type") == "applemusicopen":
                apple_music_url = action.get("uri")
                break

    result = {
        "id": track.get("key"),
        "isrc": track.get("isrc"),
        "title": track.get("title"),
        "artist": track.get("subtitle"),
        "album": metadata.get("Album"),
        "label": metadata.get("Label"),
        "released": metadata.get("Released"),
        "genre": track.get("genres", {}).get("primary"),
        "type": track.get("type"),
        "coverart": track.get("images", {}).get("coverarthq"),
        "links": {
            "shazam": track.get("url"),
            "apple_music": apple_music_url,
        },
    }

    return json.dumps(result, indent=2)

asyncio.run(main())