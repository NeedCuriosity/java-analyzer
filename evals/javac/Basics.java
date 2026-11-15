import java.util.ArrayList;
import java.util.List;

public class Basics {
    private final List<String> names = new ArrayList<>();
    private int total;

    public int add(String name, int weight) {
        int before = total;
        names.add(name);
        total = before + weight;
        return total;
    }

    public String longest() {
        String best = "";
        for (String name : names) {
            if (name.length() > best.length()) {
                best = name;
            }
        }
        return best;
    }

    public int parse(String text) throws Exception {
        if (text == null) {
            throw new Exception("null");
        }
        int value = Integer.parseInt(text.trim());
        return value * 2;
    }
}
