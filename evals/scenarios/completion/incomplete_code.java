import java.util.ArrayList;
import java.util.List;
import java.util.Map;

class Typing {
    private final List<String> names = new ArrayList<>();
    private int count;

    void ifCondition(String s) {
        if (s./*@ifDot*/
        count++;
    }

    void midArgs(String s) {
        System.out.println(s./*@argDot*/
        count++;
    }

    void initNoSemicolon() {
        String first = names./*@initDot*/
        count++;
    }

    void lambdaChain() {
        names.stream().map(n -> n./*@lambdaDot*/
    }

    void forEachHeader() {
        for (String n : names./*@forDot*/
    }

    String returnThis() {
        return this./*@thisDot*/
    }

    void newArgs() {
        List<String> copy = new ArrayList<>(names./*@newDot*/
    }

    void localThenDot() {
        Map<String, Integer> map = Map.of();
        map./*@mapDot*/
        names.clear();
    }

    void bareIdent() {
        String text = "x";
        tex/*@bare*/
        count++;
    }

    void afterReturn(String s) {
        return s.isEmpty() ? names./*@ternDot*/
    }

    int afterAll() { return count; }
}
//? complete ifDot has isEmpty length
//? complete argDot has length trim
//? complete initDot has get size
//? complete lambdaDot has length toUpperCase
//? complete forDot has subList size
//? complete thisDot has names count afterAll
//? complete newDot has size
//? complete mapDot has get put
//? complete bare has text
//? complete ternDot has size
