import java.util.ArrayList;
import java.util.List;
import java.util.StringJoiner;

public class Strings {
    public String block() {
        String text = """
            hello
              world
            """;
        String stripped = text.strip();
        return stripped;
    }

    public String build(List<String> parts) {
        StringBuilder builder = new StringBuilder();
        for (String part : parts) {
            builder.append(part).append(',');
        }
        int length = builder.length();
        if (length > 0) {
            builder.setLength(length - 1);
        }
        return builder.toString();
    }

    public List<String> split(String csv) {
        List<String> out = new ArrayList<>();
        String[] fields = csv.split(",");
        for (String field : fields) {
            String trimmed = field.trim();
            if (!trimmed.isEmpty()) {
                out.add(trimmed);
            }
        }
        return out;
    }

    public String format(String name, int count, double ratio) {
        String formatted = String.format("%s=%d (%.2f)", name, count, ratio);
        StringJoiner joiner = new StringJoiner("|", "[", "]");
        joiner.add(formatted).add(name);
        String joined = joiner.toString();
        return joined;
    }

    public int chars(String text) {
        int vowels = 0;
        for (char c : text.toCharArray()) {
            char lower = Character.toLowerCase(c);
            if ("aeiou".indexOf(lower) >= 0) {
                vowels++;
            }
        }
        int code = text.codePointAt(0);
        boolean same = text.equalsIgnoreCase("x");
        return same ? vowels : vowels + code;
    }

    public String concat(Object o, long n, char c, boolean b) {
        String all = "" + o + n + c + b + null;
        String repeated = all.repeat(2);
        CharSequence view = repeated.subSequence(0, 1);
        return view.toString();
    }
}
