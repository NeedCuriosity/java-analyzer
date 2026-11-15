import java.io.PrintStream;
import java.util.Locale;

class Main {
    static final String TEMPLATE = "%s scored %d";

    void run(String name, int score, double ratio, PrintStream out, Object[] values) {
        String.format("%s scored %d", name, score);
        String.format(/*@too_few*/"%s scored %d", name);
        String.format("%s", name, /*@too_many*/score);
        String.format(Locale.ROOT, "%.2f%%", ratio);
        String.format(/*@unknown*/"%q", name);
        String.format("%d points", /*@wrong_type*/name);
        String.format("%f", /*@int_for_f*/score);
        String.format(/*@constant*/TEMPLATE, name);
        String.format("%2$s %1$s %<s", name, name);
        String.format("%s %s", values);
        "%s-%s".formatted(name, name);
        /*@formatted*/"%s-%s".formatted(name);
        out.printf("%d%n", score);
        out.printf(/*@malformed*/"50%", score);
        String.format("%b %c %x", name, 'c', 255L);
    }
}
//? diag malformed-format-string too_few
//? diag malformed-format-string too_many
//? diag malformed-format-string unknown
//? diag malformed-format-string wrong_type
//? diag malformed-format-string int_for_f
//? diag malformed-format-string constant
//? diag malformed-format-string formatted
//? diag malformed-format-string malformed
//? diag only malformed-format-string
