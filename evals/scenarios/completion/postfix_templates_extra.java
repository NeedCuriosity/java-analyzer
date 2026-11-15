import java.util.List;

class Main {
    void flags(boolean ready, int a, int b) {
        ready.el/*@else_simple*/
    }

    void compare(int a, int b) {
        (a > b).el/*@else_paren*/
    }

    void calls(String text, Object lock, String[] words, List<String> names) {
        text.isEmpty().as/*@assert*/
    }

    void refs(String text, Object lock) {
        lock.sync/*@sync*/
    }

    void strings(String text) {
        text.form/*@format*/
    }

    void nonNull(String text) {
        text.reqn/*@reqnonnull*/
    }

    void arrays(String[] words) {
        words.ita/*@itar*/
    }

    void args(String text) {
        text.ar/*@arg*/
    }

    void err(String text) {
        text.ser/*@serr*/
    }
}
//? insert else_simple else ~ "if (!ready) {"
//? insert else_paren else ~ "if (!(a > b)) {"
//? insert assert assert ~ "assert text.isEmpty();"
//? insert sync synchronized ~ "synchronized (lock) {"
//? insert format format ~ "String.format(text, $1)"
//? insert reqnonnull reqnonnull ~ "Objects.requireNonNull(text)"
//? insert itar itar ~ "var ${2:item} = words[$1];"
//? insert arg arg ~ "${1:call}(text)"
//? insert serr serr ~ "System.err.println(text);"
