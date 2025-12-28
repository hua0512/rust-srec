
try:
    with open('d:/Develop/hua0512/stream-rec/rust-srec/rust-srec/frontend/src/locales/zh-CN/messages.po', 'r', encoding='utf-8') as f:
        lines = f.readlines()
    
    for i, line in enumerate(lines):
        if line.strip() == 'msgstr ""':
            # Check previous line for msgid
            # Note: msgid might be multi-line, but usually starts at i-1 or earlier. 
            # Simple check: if msgid is not empty string, it's a real missing translation.
            # Header has msgid "" at line 1.
            
            # Let's verify if the PREVIOUS line is 'msgid ""'
            is_header = False
            if i > 0 and lines[i-1].strip() == 'msgid ""':
                is_header = True
            
            if not is_header:
                print(f"Line {i+1}: {line.strip()}")
                # Print context (msgid)
                j = i - 1
                while j >= 0 and lines[j].startswith("msgid"):
                    print(f"  Context: {lines[j].strip()}")
                    j -= 1
except Exception as e:
    print(e)
