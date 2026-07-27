namespace Keyboop.Core.Layout;

internal static class ExtraWords
{
    public static readonly IReadOnlySet<string> RussianWords = Set(
        "тк", "тд", "тп", "тн", "тч", "др", "пр", "см", "ср", "гл", "гг",
        "шт", "кв", "эт", "оф", "пп", "зы", "напр", "прим", "примеч", "сокр",
        "рис", "табл", "стр", "разд", "итд", "итп", "чтд", "гос", "тыс", "млн",
        "млрд", "руб", "коп", "экз", "изд", "вып", "пер", "просп", "наб", "корп",
        "каб", "ауд", "тел", "факс", "имхо", "спс", "пжл", "плз", "инфа",
        "ого", "агага", "ага", "угу", "угум", "эге", "эх", "эхх", "ах", "ахах",
        "ох", "ой", "ай", "эй", "уф", "фу", "фух", "ха", "хах", "хаха", "хе",
        "хех", "хи", "хих", "опа", "упс", "ауч", "ау", "оу", "ну", "го", "че",
        "чё", "щас", "тоже", "тож", "норм", "лан", "оке", "окей", "ясн", "оки",
        "угусь", "неа", "нее", "неее", "даа", "дада", "нуу", "воу", "вау",
        "офк", "омг", "емое", "ёмое", "блин", "капец", "вайб", "вайбовый",
        "войс", "гг", "донат", "донатить", "дроп", "дропнуть", "жиза", "изи",
        "имба", "катка", "кек", "краш", "кринж", "лол", "мемчик", "пруф",
        "рандом", "рофл", "скилл", "тильт", "токсик", "топчик", "флекс",
        "хайп", "чилл", "чиллить");

    public static readonly IReadOnlySet<string> EnglishWords = Set(
        "afk", "based", "boomer", "brainrot", "bruh", "bussin", "chadlike",
        "copium", "cringey", "deadass", "delulu", "doomer", "fomo", "frfr",
        "gaslighting", "gg", "ggwp", "gigachad", "glhf", "goated", "gyat",
        "idk", "irl", "lmao", "lmfao", "lowkey", "mewing", "mid", "mogged",
        "ngl", "nocap", "noob", "npc", "omg", "pov", "rizz", "simping",
        "situationship", "skibidi", "smh", "sus", "tldr", "uwu", "vibing",
        "wow", "yolo", "zoomer");

    public static readonly IReadOnlySet<string> ForceSwapAbbreviations = Set(
        "http", "https", "url", "uri", "api", "rest", "json", "xml", "yaml",
        "csv", "html", "css", "sdk", "cli", "gui", "ide", "ssh", "ftp", "tcp",
        "udp", "ip", "dns", "vpn", "ssl", "tls", "smtp", "jwt", "cors", "sql",
        "nosql", "git", "npm", "yarn", "k8s", "aws", "gcp", "kpi", "crm", "seo",
        "smm", "mvp", "cpu", "gpu", "ram", "ssd", "hdd", "usb", "pdf", "mp3",
        "mp4", "png", "jpg", "jpeg", "svg", "gif", "ddos", "iot", "llm", "gpt",
        "ml", "ai", "ui", "ux", "db", "os", "io", "qa", "ci", "cd", "webp",
        "mvc", "orm", "cdn", "dom");

    public static readonly IReadOnlySet<string> RussianSingleLetters =
        Set("а", "в", "и", "к", "о", "с", "у", "я");

    public static readonly IReadOnlySet<string> ForceRussianAmbiguous =
        Set("ns", "yt", "pf", "bp", "nj", "kb", "yb", "nt", "nf", "ult", "lt");

    public static readonly IReadOnlySet<string> DefaultKeep =
        Set("вк", "тг", "ок", "оз", "лс", "сек", "мск", "зп", "дз", "дс",
            "ндфл", "тс");

    public static readonly IReadOnlySet<string> EnglishKeepShort = Set(
        "in", "on", "at", "to", "of", "is", "it", "if", "as", "an", "or", "by",
        "we", "he", "me", "be", "my", "up", "us", "do", "go", "no", "so", "and",
        "the", "for", "you", "ok", "tv", "ps", "vs", "dj", "gg", "id", "ip",
        "pm", "am", "hr", "pr", "ai", "uk", "eu", "cv", "afk", "ceo", "ru",
        "jar", "ns", "yt", "pf", "bp", "nj", "kb", "yb", "nt", "nf", "ult",
        "lt", "ah", "oh", "eh", "hm", "ha", "lol", "omg", "wow");

    public static readonly IReadOnlySet<string> EnglishLabelClassifiers = Set(
        "vitamin", "vitamins", "hepatitis", "gen", "generation", "grade", "grades",
        "type", "types", "series", "model", "models", "plan", "plans", "option",
        "options", "answer", "answers", "choice", "choices", "question", "questions",
        "section", "sections", "appendix", "figure", "figures", "variant", "variants",
        "size", "sizes", "tier", "phase", "blood", "omega", "chapter", "clause",
        "paragraph", "exam", "quiz", "mark", "round", "rounds", "part", "parts",
        "item", "items", "version", "class", "category", "level", "stage", "band",
        "division", "league", "point", "note", "notes", "step", "steps", "unit",
        "units");

    public static readonly IReadOnlySet<string> RussianLabelClassifiers = Set(
        "витамин", "витамины", "группа", "группы", "тип", "типа", "класс",
        "вариант", "версия", "модель", "серия", "раздел", "пункт", "номер",
        "уровень", "этап", "раунд", "часть", "рис", "таблица", "формат");

    private static HashSet<string> Set(params string[] values) =>
        values.ToHashSet(StringComparer.Ordinal);
}
